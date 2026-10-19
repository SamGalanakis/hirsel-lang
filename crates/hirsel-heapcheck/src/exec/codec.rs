//! `Inst::Encode` and `Inst::Decode` (SER-2; LIR schema revision 29) over ledger values: the
//! canonical codec reimplemented from the spec (`docs/spec/serialization.md`), since this crate
//! never depends on `hirsel-types` or the runtime. Offsets come from the layout records the plan
//! names, never from the runtime's codec program, so a lowering error disagrees here. Decoding
//! validates the whole input before it allocates; the blocks it builds are fresh, zeroed as the
//! runtime zeroes them, each held by exactly one counted cell.
use super::compare::At;
use super::*;
use hirsel_lir::codec::{CONTAINER_ENTRIES, CONTAINER_INDEX, CodecNode, CodecNodeId, CodecPlan};
use hirsel_lir::layout_types::{FieldLayout, TagEncoding};
use std::collections::BTreeSet;

const MAGIC: &[u8; 4] = b"HSC1";
const VERSION: u32 = 1;
/// The reference codec's operational value budget (`hirsel_types::wire::Limits`).
const VALUE_BUDGET: u64 = 1_000_000;

/// `DecodeError`'s ordinals, in the reference's order of detection.
#[derive(Clone, Copy, Debug)]
enum Malformed {
    Version = 0,
    SchemaMismatch = 1,
    Truncated = 2,
    Trailing = 3,
    Scalar = 4,
    Utf8 = 5,
    Tag = 6,
    Length = 7,
    Duplicate = 8,
    Order = 9,
    /// A check rejected a decoded representation (SER-1).
    Invalid = 10,
}

struct Input<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Input<'a> {
    fn take(&mut self, n: u64) -> Result<&'a [u8], Malformed> {
        let end = (self.at as u64).checked_add(n).ok_or(Malformed::Length)?;
        if end > self.bytes.len() as u64 {
            return Err(Malformed::Truncated);
        }
        let out = &self.bytes[self.at..end as usize];
        self.at = end as usize;
        Ok(out)
    }
    fn number(&mut self, n: u64) -> Result<u128, Malformed> {
        let mut b = [0u8; 16];
        b[..n as usize].copy_from_slice(self.take(n)?);
        Ok(u128::from_le_bytes(b))
    }
}

/// Where a decoded part is written: the result image, or a fresh allocation.
#[derive(Clone, Copy)]
enum Dest {
    Image(u64),
    Heap(Ptr),
}

impl Dest {
    fn at(self, offset: u64) -> Self {
        match self {
            Dest::Image(o) => Dest::Image(o + offset),
            Dest::Heap(p) => Dest::Heap(p.at(offset)),
        }
    }
}

fn nan32(bits: u32) -> bool {
    bits & 0x7fff_ffff > 0x7f80_0000
}
fn nan64(bits: u64) -> bool {
    bits & 0x7fff_ffff_ffff_ffff > 0x7ff0_0000_0000_0000
}

impl<S: Stage> Machine<'_, S> {
    fn plan_node<'p>(&self, plan: &'p CodecPlan, id: CodecNodeId) -> Checked<&'p CodecNode> {
        plan.node(id).map_err(|e| err(V::Malformed, e))
    }

    fn field_at(&self, layout: LayoutId, ordinal: u32) -> Checked<FieldLayout> {
        self.record(layout)?
            .fields
            .iter()
            .find(|f| f.ordinal == ordinal)
            .cloned()
            .ok_or_else(|| err(V::Malformed, format!("codec field {ordinal} of {layout:?}")))
    }

    /// The fields of arm `ordinal` of the variant `layout`, by field ordinal.
    fn arm_fields(&self, layout: LayoutId, ordinal: u32) -> Checked<(TagEncoding, Vec<u64>)> {
        let tag = self
            .record(layout)?
            .tag
            .clone()
            .ok_or_else(|| err(V::Malformed, "a codec variant without a tag"))?;
        let variant = tag
            .variants
            .iter()
            .find(|v| v.ordinal == ordinal)
            .ok_or_else(|| err(V::Malformed, format!("codec arm {ordinal}")))?;
        let mut offsets = vec![];
        for i in 0..variant.fields.len() as u32 {
            let f = variant
                .fields
                .iter()
                .find(|f| f.ordinal == i)
                .ok_or_else(|| err(V::Malformed, "a codec arm field"))?;
            offsets.push(f.offset);
        }
        Ok((tag, offsets))
    }

    /// The payload of a block layout.
    fn block_payload(&self, layout: LayoutId) -> Checked<LayoutId> {
        match self.record(layout)?.kind {
            LayoutKind::Block { payload } | LayoutKind::Box { payload } => Ok(payload),
            ref kind => fail(V::TypeConfusion, format!("a codec block of {kind:?}")),
        }
    }

    /// A sequence block layout's payload, element, first element offset and stride.
    fn codec_sequence(&self, layout: LayoutId) -> Checked<(LayoutId, LayoutId, u64, u64)> {
        let payload = self.block_payload(layout)?;
        let LayoutKind::Sequence { element } = self.record(payload)?.kind else {
            return fail(V::TypeConfusion, "a codec sequence of another payload");
        };
        let (start, stride) = self.sequence_shape(element)?;
        Ok((payload, element, start, stride))
    }

    /// Native flat field addresses include every preceding variable field's
    /// content extent; layout offsets already include its eight-byte prefix.
    fn codec_field_offsets(
        &self,
        layout: LayoutId,
        fields: &[FieldLayout],
        at: At<'_>,
    ) -> Checked<Vec<u64>> {
        let r = self.record(layout)?;
        let flat = r.kind == LayoutKind::Flat || r.kind == LayoutKind::Packed && r.align == 1;
        let mut extra = 0;
        let mut offsets = Vec::with_capacity(fields.len());
        for f in fields {
            let offset = f.offset + extra;
            offsets.push(offset);
            if flat && matches!(self.record(f.layout)?.size, SizeSpec::Dynamic(_)) {
                extra += self.bits_at(at.at(offset), 8)? as u64;
            }
        }
        Ok(offsets)
    }

    /// The image of a root operand of `layout`, so every node reads at an `At`.
    fn codec_image(&self, val: &Val, layout: LayoutId) -> Checked<Extent> {
        Ok(match val {
            Val::Image(e) => e.clone(),
            Val::Unit => Extent::new(0),
            Val::Bits { bits, .. } => {
                let size = self.size(layout)?;
                let mut e = Extent::new(size);
                e.write_bytes(0, &bits.to_le_bytes()[..size as usize]);
                e
            }
            Val::Ptr(p) => {
                let mut e = Extent::new(self.word);
                e.put_cell(
                    0,
                    self.word,
                    Cell::Data {
                        ptr: *p,
                        counted: false,
                        borrow: None,
                    },
                );
                e
            }
            other => return fail(V::TypeConfusion, format!("a codec operand {other:?}")),
        })
    }

    /// `Inst::Encode`: the envelope of `val`, as a fresh owned Bytes image.
    pub(super) fn codec_encode(&mut self, plan: &CodecPlan, val: &Val) -> Checked<Val> {
        let root_layout = self.plan_node(plan, plan.root)?.layout();
        let image = self.codec_image(val, root_layout)?;
        let out = self.codec_payload(plan, At::Image(&image, 0), None)?;
        let mut envelope = MAGIC.to_vec();
        envelope.extend_from_slice(&VERSION.to_le_bytes());
        envelope.extend_from_slice(&(plan.descriptor.len() as u64).to_le_bytes());
        envelope.extend_from_slice(&plan.descriptor);
        envelope.extend_from_slice(&(out.len() as u64).to_le_bytes());
        envelope.extend_from_slice(&out);
        let bytes = self.plan_node(plan, plan.bytes)?;
        let mut image = Extent::new(self.size(bytes.layout())?);
        image.write_bytes(0, &vec![0; image.len() as usize]);
        self.codec_text(&mut image, Dest::Image(0), bytes, &envelope)?;
        Ok(Val::Image(image))
    }

    /// The SER-2 payload of the root value at `root`. With `checks`, also each checked
    /// occurrence (SER-1) as its check's index and a copy of its bytes, an inner occurrence
    /// before the one holding it and otherwise in encoding order.
    fn codec_payload(
        &mut self,
        plan: &CodecPlan,
        root: At<'_>,
        mut checks: Option<&mut Vec<(u32, Extent)>>,
    ) -> Checked<Vec<u8>> {
        let mut out = vec![];
        enum Job<'e> {
            Visit(CodecNodeId, At<'e>),
            Check(u32, LayoutId, At<'e>),
            Mark,
            SetEnd,
        }
        let mut work = vec![Job::Visit(plan.root, root)];
        let mut sets: Vec<(usize, Vec<usize>)> = vec![];
        while let Some(job) = work.pop() {
            let (id, at) = match job {
                Job::Visit(id, at) => (id, at),
                Job::Check(check, layout, at) => {
                    let size = self.size(layout)?;
                    let (e, offset) = self.extent_at(at)?;
                    e.check_range(offset, size)?;
                    if !e.all_init(offset, size) {
                        return fail(V::UninitializedRead, "a checked codec value");
                    }
                    if e.cells.range(offset..offset + size).next().is_some() {
                        return fail(V::Unsupported, "a checked codec value holding references");
                    }
                    let mut copy = Extent::new(size);
                    copy.write_bytes(0, e.read_bytes(offset, size));
                    if let Some(checks) = checks.as_deref_mut() {
                        checks.push((check, copy));
                    }
                    continue;
                }
                Job::Mark => {
                    let top = sets
                        .last_mut()
                        .ok_or_else(|| err(V::Malformed, "a set mark"))?;
                    top.1.push(out.len());
                    continue;
                }
                Job::SetEnd => {
                    let (start, mut starts) =
                        sets.pop().ok_or_else(|| err(V::Malformed, "a set end"))?;
                    starts.push(out.len());
                    let mut elements: Vec<Vec<u8>> = starts
                        .windows(2)
                        .map(|w| out[w[0]..w[1]].to_vec())
                        .collect();
                    elements.sort();
                    out.truncate(start);
                    elements.iter().for_each(|e| out.extend_from_slice(e));
                    continue;
                }
            };
            let node = self.plan_node(plan, id)?;
            let layout = node.layout();
            match node {
                CodecNode::Scalar(_) | CodecNode::Char(_) => {
                    let LayoutKind::Scalar(kind) = self.record(layout)?.kind else {
                        return fail(V::TypeConfusion, "a codec scalar off its layout");
                    };
                    let size = self.size(layout)?;
                    let bits = self.bits_at(at, size)?;
                    let bits = match kind {
                        ScalarKind::Bool => u128::from(bits != 0),
                        ScalarKind::F32 if nan32(bits as u32) => 0x7fc0_0000,
                        ScalarKind::F64 if nan64(bits as u64) => 0x7ff8_0000_0000_0000,
                        _ => bits,
                    };
                    out.extend_from_slice(&bits.to_le_bytes()[..size as usize]);
                }
                CodecNode::Text { .. } => {
                    let (e, offset) = self.extent_at(at)?;
                    let bytes = self.slice_content(e, offset, layout)?;
                    out.extend_from_slice(&(bytes.len() as u64).to_le_bytes());
                    out.extend_from_slice(&bytes);
                }
                CodecNode::Path(_) => {
                    if self.record(layout)?.kind != LayoutKind::Path {
                        return fail(V::TypeConfusion, "a codec path off its layout");
                    }
                    let id = self.bits_at(at, 8)? as u64;
                    let text = self
                        .ops
                        .path_text(id)
                        .ok_or_else(|| err(V::Malformed, "unknown Path ID"))?;
                    out.extend_from_slice(&(text.len() as u64).to_le_bytes());
                    out.extend_from_slice(text.as_bytes());
                }
                CodecNode::FlatText { .. } => {
                    let length = self.bits_at(at, 8)? as u64;
                    let (e, offset) = self.extent_at(at.at(8))?;
                    e.check_range(offset, length)?;
                    if !e.all_init(offset, length) {
                        return fail(V::UninitializedRead, "flat codec text");
                    }
                    out.extend_from_slice(&length.to_le_bytes());
                    out.extend_from_slice(e.read_bytes(offset, length));
                }
                CodecNode::Framed { value, .. } => work.push(Job::Visit(*value, at.at(8))),
                CodecNode::Checked { check, value, .. } => {
                    if checks.is_some() {
                        work.push(Job::Check(*check, layout, at));
                    }
                    work.push(Job::Visit(*value, at));
                }
                CodecNode::Fields { fields, .. } => {
                    let layout_fields: Vec<_> = (0..fields.len())
                        .map(|i| self.field_at(layout, i as u32))
                        .collect::<Checked<_>>()?;
                    let offsets = self.codec_field_offsets(layout, &layout_fields, at)?;
                    for (child, offset) in fields.iter().zip(offsets).rev() {
                        work.push(Job::Visit(*child, at.at(offset)));
                    }
                }
                CodecNode::Variant { arms, .. } => {
                    let tag = self
                        .record(layout)?
                        .tag
                        .clone()
                        .ok_or_else(|| err(V::Malformed, "a codec variant without a tag"))?;
                    let ordinal = self.tag_at(at.at(tag.offset), tag.form)?;
                    let arm = arms
                        .get(ordinal as usize)
                        .ok_or_else(|| err(V::TypeConfusion, "a variant value off its arms"))?;
                    out.extend_from_slice(&ordinal.to_le_bytes());
                    let variant = tag
                        .variants
                        .iter()
                        .find(|v| v.ordinal == ordinal)
                        .ok_or_else(|| err(V::Malformed, "codec variant"))?;
                    let offsets = self.codec_field_offsets(layout, &variant.fields, at)?;
                    for (child, offset) in arm.iter().zip(offsets).rev() {
                        work.push(Job::Visit(*child, at.at(offset)));
                    }
                }
                CodecNode::Box { payload, .. } => {
                    work.push(Job::Visit(*payload, At::Heap(self.ptr_at(at)?)));
                }
                CodecNode::Sequence { element, .. } => {
                    if self.record(layout)?.kind == LayoutKind::Flat {
                        let (start, _) = self
                            .flat_prefixed(layout)?
                            .ok_or_else(|| err(V::Malformed, "codec flat sequence"))?;
                        let block = match at {
                            At::Heap(p) => p,
                            _ => return fail(V::TypeConfusion, "flat sequence outside a block"),
                        };
                        let count = self.flat_count(layout, block)?;
                        out.extend_from_slice(&count.to_le_bytes());
                        let mut cursor = start;
                        let child_layout = self.plan_node(plan, *element)?.layout();
                        let mut children = Vec::new();
                        for _ in 0..count {
                            children.push(Job::Visit(*element, At::Heap(block.at(cursor))));
                            cursor += self.extent_of(child_layout, block.at(cursor))?;
                        }
                        work.extend(children.into_iter().rev());
                    } else {
                        let p = self.block_payload(layout)?;
                        let block = self.ptr_at(at)?;
                        let count = self.bits_at(At::Heap(block), 8)? as u64;
                        out.extend_from_slice(&count.to_le_bytes());
                        match self.record(p)?.kind {
                            LayoutKind::Sequence { .. } => {
                                let (_, _, start, stride) = self.codec_sequence(layout)?;
                                for i in (0..count).rev() {
                                    work.push(Job::Visit(
                                        *element,
                                        At::Heap(block.at(start + i * stride)),
                                    ));
                                }
                            }
                            LayoutKind::Sequential { element: e } => {
                                let mut cursor =
                                    hirsel_lir::layout::sequential_elements_offset(self.record(e)?)
                                        .map_err(|e| err(V::Malformed, format!("{e:?}")))?;
                                let mut children = Vec::new();
                                for _ in 0..count {
                                    children.push(Job::Visit(*element, At::Heap(block.at(cursor))));
                                    cursor += self.extent_of(e, block.at(cursor))?;
                                }
                                work.extend(children.into_iter().rev());
                            }
                            LayoutKind::SoaColumns { element: e } => {
                                let CodecNode::Fields { fields, .. } =
                                    self.plan_node(plan, *element)?
                                else {
                                    return fail(V::Malformed, "codec column element");
                                };
                                let capacity = self.bits_at(At::Heap(block.at(8)), 8)? as u64;
                                let (data, _, columns) = self.column_shape(e)?;
                                for i in (0..count).rev() {
                                    for (child, (prefix, stride)) in
                                        fields.iter().zip(&columns).rev()
                                    {
                                        work.push(Job::Visit(
                                            *child,
                                            At::Heap(
                                                block.at(data + capacity * prefix + i * stride),
                                            ),
                                        ));
                                    }
                                }
                            }
                            _ => return fail(V::TypeConfusion, "codec sequence payload"),
                        }
                    }
                }
                CodecNode::Repeat { element, .. } => {
                    let (e, count) = match self.record(layout)?.kind {
                        LayoutKind::Inline { element, count } => (element, count),
                        LayoutKind::Simd { element, lanes } => (element, u64::from(lanes)),
                        _ => return fail(V::TypeConfusion, "a codec repeat off its layout"),
                    };
                    let stride = self.size(e)?;
                    for i in (0..count).rev() {
                        work.push(Job::Visit(*element, at.at(i * stride)));
                    }
                }
                CodecNode::Map { .. } | CodecNode::Set { .. } => {
                    let container = self.block_payload(layout)?;
                    let entries = self.field_at(container, CONTAINER_ENTRIES)?;
                    let (_, pair, start, stride) = self.codec_sequence(entries.layout)?;
                    let block = self.ptr_at(At::Heap(self.ptr_at(at)?.at(entries.offset)))?;
                    let count = self.bits_at(At::Heap(block), 8)? as u64;
                    let live = if matches!(node, CodecNode::Map { .. }) {
                        let live_field = self.field_at(container, 3)?;
                        let root = self.ptr_at(at)?;
                        self.bits_at(At::Heap(root.at(live_field.offset)), 8)? as u64
                    } else {
                        count
                    };
                    out.extend_from_slice(&live.to_le_bytes());
                    match node {
                        CodecNode::Map { key, value, .. } => {
                            let tag = self
                                .record(pair)?
                                .tag
                                .as_ref()
                                .ok_or_else(|| err(V::Malformed, "a map entry tag"))?;
                            let data = tag
                                .variants
                                .iter()
                                .find(|v| v.ordinal == 1)
                                .and_then(|v| v.fields.first())
                                .ok_or_else(|| err(V::Malformed, "a map entry pair"))?;
                            let (k, v) = (
                                self.field_at(data.layout, 0)?,
                                self.field_at(data.layout, 1)?,
                            );
                            for i in (0..count).rev() {
                                let entry = block.at(start + i * stride);
                                if self.tag_at(At::Heap(entry.at(tag.offset)), tag.form)? != 1 {
                                    continue;
                                }
                                work.push(Job::Visit(
                                    *value,
                                    At::Heap(entry.at(data.offset + v.offset)),
                                ));
                                work.push(Job::Visit(
                                    *key,
                                    At::Heap(entry.at(data.offset + k.offset)),
                                ));
                            }
                        }
                        CodecNode::Set { element, .. } => {
                            sets.push((out.len(), vec![]));
                            work.push(Job::SetEnd);
                            for i in (0..count).rev() {
                                work.push(Job::Visit(
                                    *element,
                                    At::Heap(block.at(start + i * stride)),
                                ));
                                work.push(Job::Mark);
                            }
                        }
                        _ => unreachable!(),
                    }
                }
            }
        }
        Ok(out)
    }

    /// A fresh byte sequence of `bytes` as the slice of `node` at `dest`.
    fn codec_text(
        &mut self,
        image: &mut Extent,
        dest: Dest,
        node: &CodecNode,
        bytes: &[u8],
    ) -> Checked<()> {
        let CodecNode::Text {
            layout, storage, ..
        } = node
        else {
            return fail(V::Malformed, "a codec text node");
        };
        let LayoutKind::Slice { element } = self.record(*layout)?.kind else {
            return fail(V::TypeConfusion, "a codec text off a slice");
        };
        let (header, whole) = self.byte_sequence(element, bytes)?;
        let first = header.len() as u64;
        let owner = self.heap_alloc(
            *storage,
            first + bytes.len() as u64,
            self.record(*storage)?.align,
        )?;
        let extent = &mut self.ledger.get_mut(owner.alloc).extent;
        extent.write_bytes(0, &header);
        extent.write_bytes(first, bytes);
        let mut slice = self.slice_image(*layout, owner, whole, bytes.len() as u64)?;
        for cell in slice.cells.values_mut() {
            if let Cell::Data { counted, .. } = cell {
                *counted = true;
            }
        }
        self.codec_place(image, dest, slice);
        Ok(())
    }

    fn codec_place(&mut self, image: &mut Extent, dest: Dest, part: Extent) {
        match dest {
            Dest::Image(o) => image.place(o, part),
            Dest::Heap(p) => self.ledger.get_mut(p.alloc).extent.place(p.offset, part),
        }
    }

    fn codec_bytes(&mut self, image: &mut Extent, dest: Dest, bytes: &[u8]) {
        match dest {
            Dest::Image(o) => image.write_bytes(o, bytes),
            Dest::Heap(p) => self
                .ledger
                .get_mut(p.alloc)
                .extent
                .write_bytes(p.offset, bytes),
        }
    }

    fn codec_cell(&mut self, image: &mut Extent, dest: Dest, ptr: Ptr) {
        let cell = Cell::Data {
            ptr,
            counted: true,
            borrow: None,
        };
        match dest {
            Dest::Image(o) => image.put_cell(o, self.word, cell),
            Dest::Heap(p) => self
                .ledger
                .get_mut(p.alloc)
                .extent
                .put_cell(p.offset, self.word, cell),
        }
    }

    /// A fresh zeroed heap block of `size` bytes of `layout`.
    fn codec_block(&mut self, layout: LayoutId, size: u64, align: u32) -> Checked<Ptr> {
        let block = self.heap_alloc(layout, size, align)?;
        self.ledger
            .get_mut(block.alloc)
            .extent
            .write_bytes(0, &vec![0; size as usize]);
        Ok(block)
    }

    /// A fresh sequence block of the sequence block layout `layout`, holding `count`
    /// zeroed elements.
    fn codec_elements(&mut self, layout: LayoutId, count: u64) -> Checked<(Ptr, u64, u64)> {
        let (payload, _, start, stride) = self.codec_sequence(layout)?;
        let block =
            self.codec_block(payload, start + count * stride, self.record(payload)?.align)?;
        let extent = &mut self.ledger.get_mut(block.alloc).extent;
        extent.write_bytes(0, &count.to_le_bytes());
        extent.write_bytes(
            hirsel_lir::layout::SEQUENCE_CAPACITY_OFFSET,
            &count.to_le_bytes(),
        );
        Ok((block, start, stride))
    }

    /// The reference's validation of a complete envelope, in its reading order.
    fn codec_validate<'b>(
        &self,
        plan: &CodecPlan,
        bytes: &'b [u8],
    ) -> Checked<Result<&'b [u8], Malformed>> {
        let mut input = Input { bytes, at: 0 };
        let envelope = |input: &mut Input<'b>| -> Result<&'b [u8], Malformed> {
            if input.take(4)? != MAGIC || input.number(4)? != u128::from(VERSION) {
                return Err(Malformed::Version);
            }
            let n = input.number(8)? as u64;
            if input.take(n)? != plan.descriptor.as_slice() {
                return Err(Malformed::SchemaMismatch);
            }
            let n = input.number(8)? as u64;
            let payload = input.take(n)?;
            if input.at != input.bytes.len() {
                return Err(Malformed::Trailing);
            }
            Ok(payload)
        };
        let payload = match envelope(&mut input) {
            Ok(payload) => payload,
            Err(e) => return Ok(Err(e)),
        };
        enum Job {
            Visit(CodecNodeId),
            /// The next `count` elements; `key`: each one a key of the innermost set.
            Elements {
                node: CodecNodeId,
                count: u64,
                key: bool,
            },
            Entries {
                key: CodecNodeId,
                value: CodecNodeId,
                count: u64,
            },
            KeyEnd(usize, bool),
            Open,
            Close,
        }
        let mut input = Input {
            bytes: payload,
            at: 0,
        };
        let mut work = vec![Job::Visit(plan.root)];
        let mut open: Vec<(BTreeSet<&[u8]>, Option<&[u8]>)> = vec![];
        // The reference's operational value budget: past it, H declines.
        let mut values = 0u64;
        let budget = |values: u64, n: u64| {
            if n > VALUE_BUDGET.saturating_sub(values) {
                return fail(
                    V::Unsupported,
                    "a codec value beyond the operational budget",
                );
            }
            Ok(())
        };
        macro_rules! check {
            ($e:expr) => {
                match $e {
                    Ok(v) => v,
                    Err(e) => return Ok(Err(e)),
                }
            };
        }
        while let Some(job) = work.pop() {
            let id = match job {
                Job::Visit(id) => id,
                Job::Open => {
                    open.push((BTreeSet::new(), None));
                    continue;
                }
                Job::Close => {
                    open.pop();
                    continue;
                }
                Job::Elements { node, count, key } => {
                    if count > 0 {
                        work.push(Job::Elements {
                            node,
                            count: count - 1,
                            key,
                        });
                        if key {
                            work.push(Job::KeyEnd(input.at, true));
                        }
                        work.push(Job::Visit(node));
                    }
                    continue;
                }
                Job::Entries { key, value, count } => {
                    if count > 0 {
                        work.push(Job::Entries {
                            key,
                            value,
                            count: count - 1,
                        });
                        work.push(Job::Visit(value));
                        work.push(Job::KeyEnd(input.at, false));
                        work.push(Job::Visit(key));
                    }
                    continue;
                }
                Job::KeyEnd(at, set) => {
                    let key = &input.bytes[at..input.at];
                    let (keys, previous) = open
                        .last_mut()
                        .ok_or_else(|| err(V::Malformed, "a codec key outside a collection"))?;
                    if set {
                        if let Some(p) = *previous {
                            if p == key {
                                return Ok(Err(Malformed::Duplicate));
                            }
                            if p > key {
                                return Ok(Err(Malformed::Order));
                            }
                        }
                        *previous = Some(key);
                    } else if !keys.insert(key) {
                        return Ok(Err(Malformed::Duplicate));
                    }
                    continue;
                }
            };
            values += 1;
            budget(values, 0)?;
            let node = self.plan_node(plan, id)?;
            let layout = node.layout();
            match node {
                CodecNode::Scalar(_) | CodecNode::Char(_) => {
                    let LayoutKind::Scalar(kind) = self.record(layout)?.kind else {
                        return fail(V::TypeConfusion, "a codec scalar off its layout");
                    };
                    let size = self.size(layout)?;
                    let bits = check!(input.number(size));
                    let bad = match kind {
                        ScalarKind::Bool => bits > 1,
                        ScalarKind::F32 => nan32(bits as u32) && bits != 0x7fc0_0000,
                        ScalarKind::F64 => nan64(bits as u64) && bits != 0x7ff8_0000_0000_0000,
                        _ => false,
                    } || matches!(node, CodecNode::Char(_))
                        && char::from_u32(bits as u32).is_none();
                    if bad {
                        return Ok(Err(Malformed::Scalar));
                    }
                }
                CodecNode::Text { utf8, .. } | CodecNode::FlatText { utf8, .. } => {
                    let n = check!(input.number(8)) as u64;
                    let text = check!(input.take(n));
                    if *utf8 && std::str::from_utf8(text).is_err() {
                        return Ok(Err(Malformed::Utf8));
                    }
                }
                // `path.of` admits any text.
                CodecNode::Path(_) => {
                    let n = check!(input.number(8)) as u64;
                    if std::str::from_utf8(check!(input.take(n))).is_err() {
                        return Ok(Err(Malformed::Utf8));
                    }
                }
                CodecNode::Fields { fields, .. } => {
                    work.extend(fields.iter().rev().map(|f| Job::Visit(*f)));
                }
                CodecNode::Variant { arms, .. } => {
                    let tag = check!(input.number(4)) as usize;
                    let Some(arm) = arms.get(tag) else {
                        return Ok(Err(Malformed::Tag));
                    };
                    work.extend(arm.iter().rev().map(|f| Job::Visit(*f)));
                }
                CodecNode::Framed { value, .. } | CodecNode::Checked { value, .. } => {
                    work.push(Job::Visit(*value))
                }
                CodecNode::Box { payload, .. } => work.push(Job::Visit(*payload)),
                CodecNode::Sequence { element, .. } => {
                    let count = check!(input.number(8)) as u64;
                    budget(values, count)?;
                    work.push(Job::Elements {
                        node: *element,
                        count,
                        key: false,
                    });
                }
                CodecNode::Repeat { element, .. } => {
                    let count = match self.record(layout)?.kind {
                        LayoutKind::Inline { count, .. } => count,
                        LayoutKind::Simd { lanes, .. } => u64::from(lanes),
                        _ => return fail(V::TypeConfusion, "a codec repeat off its layout"),
                    };
                    budget(values, count)?;
                    work.push(Job::Elements {
                        node: *element,
                        count,
                        key: false,
                    });
                }
                CodecNode::Map { key, value, .. } => {
                    let count = check!(input.number(8)) as u64;
                    budget(values, count.saturating_mul(2))?;
                    work.push(Job::Close);
                    work.push(Job::Entries {
                        key: *key,
                        value: *value,
                        count,
                    });
                    work.push(Job::Open);
                }
                CodecNode::Set { element, .. } => {
                    let count = check!(input.number(8)) as u64;
                    budget(values, count)?;
                    work.push(Job::Close);
                    work.push(Job::Elements {
                        node: *element,
                        count,
                        key: true,
                    });
                    work.push(Job::Open);
                }
            }
        }
        if input.at != payload.len() {
            return Ok(Err(Malformed::Trailing));
        }
        Ok(Ok(payload))
    }

    /// `Inst::Decode`: `Ok` of fresh owned data or `Error` of the first malformation.
    /// The decoded `Result[T, DecodeError]` and, for an `Ok`, each checked occurrence in it
    /// still to check (SER-1), in the order the checks run.
    pub(super) fn codec_decode(
        &mut self,
        plan: &CodecPlan,
        val: &Val,
    ) -> Checked<(Val, Vec<(u32, Extent)>)> {
        let bytes_layout = self.plan_node(plan, plan.bytes)?.layout();
        let bytes = self.slice_bytes(val, bytes_layout)?;
        let result = plan
            .result
            .ok_or_else(|| err(V::Malformed, "a decode plan without a result"))?;
        let result_node = self.plan_node(plan, result)?;
        let result_layout = result_node.layout();
        let mut image = Extent::new(self.size(result_layout)?);
        image.write_bytes(0, &vec![0; image.len() as usize]);
        let CodecNode::Variant { arms, .. } = result_node else {
            return fail(V::Malformed, "a decode result that is not a variant");
        };
        let checked = self.codec_validate(plan, &bytes)?;
        let (ordinal, field, payload) = match checked {
            Ok(payload) => (0u32, arms[0][0], Some(payload.to_vec())),
            Err(error) => (1, arms[1][0], {
                let error_layout = self.plan_node(plan, arms[1][0])?.layout();
                let (_, offsets) = self.arm_fields(result_layout, 1)?;
                let (tag, _) = self.arm_fields(error_layout, error as u32)?;
                self.codec_tag(&mut image, Dest::Image(offsets[0]), &tag, error as u32);
                None
            }),
        };
        let (tag, offsets) = self.arm_fields(result_layout, ordinal)?;
        self.codec_tag(&mut image, Dest::Image(0), &tag, ordinal);
        let mut checks = vec![];
        if let Some(payload) = payload {
            self.codec_build(plan, &mut image, Dest::Image(offsets[0]), field, &payload)?;
            if !plan.checks.is_empty() {
                self.codec_payload(plan, At::Image(&image, offsets[0]), Some(&mut checks))?;
            }
        }
        Ok((Val::Image(image), checks))
    }

    /// `Error(Invalid)` of a decode plan's result, and the `Ok` payload's layout and offset
    /// in the rejected image, which the caller releases.
    pub(super) fn codec_invalid(&mut self, plan: &CodecPlan) -> Checked<(Val, LayoutId, u64)> {
        let result = plan
            .result
            .ok_or_else(|| err(V::Malformed, "a decode plan without a result"))?;
        let result_node = self.plan_node(plan, result)?;
        let result_layout = result_node.layout();
        let CodecNode::Variant { arms, .. } = result_node else {
            return fail(V::Malformed, "a decode result that is not a variant");
        };
        let ok_layout = self.plan_node(plan, arms[0][0])?.layout();
        let (_, ok_offsets) = self.arm_fields(result_layout, 0)?;
        let mut image = Extent::new(self.size(result_layout)?);
        image.write_bytes(0, &vec![0; image.len() as usize]);
        let error_layout = self.plan_node(plan, arms[1][0])?.layout();
        let (_, offsets) = self.arm_fields(result_layout, 1)?;
        let invalid = Malformed::Invalid as u32;
        let (tag, _) = self.arm_fields(error_layout, invalid)?;
        self.codec_tag(&mut image, Dest::Image(offsets[0]), &tag, invalid);
        let (tag, _) = self.arm_fields(result_layout, 1)?;
        self.codec_tag(&mut image, Dest::Image(0), &tag, 1);
        Ok((Val::Image(image), ok_layout, ok_offsets[0]))
    }

    /// Write `ordinal`'s tag; a niche's nullary arm is the zero word already there.
    fn codec_tag(&mut self, image: &mut Extent, dest: Dest, tag: &TagEncoding, ordinal: u32) {
        if matches!(tag.form, TagForm::Ordinal) {
            self.codec_bytes(image, dest.at(tag.offset), &ordinal.to_le_bytes());
        }
    }

    /// Ordinary Packed elements retain native offsets and zeroed padding,
    /// independently of flat framing and the runtime's arm extent constants.
    fn codec_inline_build(
        &self,
        plan: &CodecPlan,
        root: CodecNodeId,
        input: &mut Input<'_>,
    ) -> Checked<Vec<u8>> {
        let layout = self.plan_node(plan, root)?.layout();
        let mut bytes = vec![0; self.size(layout)? as usize];
        let mut work = vec![(root, 0usize)];
        let reread = |_| err(V::Malformed, "Packed codec reread");
        while let Some((id, at)) = work.pop() {
            let node = self.plan_node(plan, id)?;
            let layout = node.layout();
            if self.record(layout)?.kind == LayoutKind::Flat {
                let part = self.codec_flat_build(plan, id, input)?;
                bytes[at..at + part.len()].copy_from_slice(&part);
                continue;
            }
            match node {
                CodecNode::Scalar(_) | CodecNode::Char(_) => {
                    let n = self.size(layout)? as usize;
                    bytes[at..at + n].copy_from_slice(input.take(n as u64).map_err(reread)?);
                }
                CodecNode::Fields { fields, .. } => {
                    for (i, child) in fields.iter().enumerate().rev() {
                        work.push((
                            *child,
                            at + self.field_at(layout, i as u32)?.offset as usize,
                        ));
                    }
                }
                CodecNode::Variant { arms, .. } => {
                    let ordinal = input.number(4).map_err(reread)? as u32;
                    let (tag, offsets) = self.arm_fields(layout, ordinal)?;
                    let offset = at + tag.offset as usize;
                    bytes[offset..offset + 4].copy_from_slice(&ordinal.to_le_bytes());
                    for (child, offset) in arms[ordinal as usize].iter().zip(offsets).rev() {
                        work.push((*child, at + offset as usize));
                    }
                }
                CodecNode::Checked { value, .. } => work.push((*value, at)),
                CodecNode::Repeat { element, .. } => {
                    let LayoutKind::Inline { count, element: e } = self.record(layout)?.kind else {
                        return fail(V::Malformed, "Packed inline codec");
                    };
                    for i in (0..count).rev() {
                        work.push((*element, at + (i * self.size(e)?) as usize));
                    }
                }
                _ => return fail(V::Malformed, "reference-bearing Packed codec element"),
            }
        }
        let extent = hirsel_lir::layout::evaluate_size(&self.record(layout)?.size, &bytes)
            .map_err(|e| err(V::Malformed, format!("{e:?}")))?;
        bytes.truncate(extent as usize);
        Ok(bytes)
    }

    /// Rebuild private flat bytes from validated canonical fields. Prefix
    /// patching is independent of the runtime's codec program and size walk.
    fn codec_flat_build(
        &self,
        plan: &CodecPlan,
        root: CodecNodeId,
        input: &mut Input<'_>,
    ) -> Checked<Vec<u8>> {
        enum Job {
            Visit(CodecNodeId),
            Elements(CodecNodeId, u64),
            Prefix(usize),
        }
        let mut work = vec![Job::Visit(root)];
        let mut bytes = Vec::new();
        let reread = |_| err(V::Malformed, "flat codec reread");
        while let Some(job) = work.pop() {
            let id = match job {
                Job::Visit(id) => id,
                Job::Prefix(at) => {
                    let n = (bytes.len() - at - 8) as u64;
                    bytes[at..at + 8].copy_from_slice(&n.to_le_bytes());
                    continue;
                }
                Job::Elements(id, count) => {
                    if count > 0 {
                        work.push(Job::Elements(id, count - 1));
                        work.push(Job::Visit(id));
                    }
                    continue;
                }
            };
            let node = self.plan_node(plan, id)?;
            match node {
                CodecNode::Scalar(layout) | CodecNode::Char(layout) => {
                    bytes.extend_from_slice(input.take(self.size(*layout)?).map_err(reread)?)
                }
                CodecNode::FlatText { .. } => {
                    let n = input.number(8).map_err(reread)? as u64;
                    bytes.extend_from_slice(&n.to_le_bytes());
                    bytes.extend_from_slice(input.take(n).map_err(reread)?);
                }
                CodecNode::Framed { value, .. } => {
                    work.push(Job::Prefix(bytes.len()));
                    bytes.extend_from_slice(&[0; 8]);
                    work.push(Job::Visit(*value));
                }
                CodecNode::Fields { fields, .. } => {
                    work.extend(fields.iter().rev().map(|&id| Job::Visit(id)))
                }
                CodecNode::Checked { value, .. } => work.push(Job::Visit(*value)),
                CodecNode::Variant { arms, .. } => {
                    let ordinal = input.number(4).map_err(reread)? as u32;
                    bytes.extend_from_slice(&ordinal.to_le_bytes());
                    work.extend(
                        arms[ordinal as usize]
                            .iter()
                            .rev()
                            .map(|&id| Job::Visit(id)),
                    );
                }
                CodecNode::Sequence { layout, element } => {
                    let n = input.number(8).map_err(reread)? as u64;
                    let (at, _) = self
                        .flat_prefixed(*layout)?
                        .ok_or_else(|| err(V::Malformed, "flat codec sequence"))?;
                    work.push(Job::Prefix(bytes.len()));
                    bytes.extend_from_slice(&[0; 8]);
                    if at == 16 {
                        bytes.extend_from_slice(&n.to_le_bytes());
                    }
                    work.push(Job::Elements(*element, n));
                }
                CodecNode::Repeat { layout, element } => {
                    let LayoutKind::Inline { count, .. } = self.record(*layout)?.kind else {
                        return fail(V::Malformed, "flat codec repeat");
                    };
                    work.push(Job::Elements(*element, count));
                }
                _ => return fail(V::Malformed, "non-flat codec node in flat build"),
            }
        }
        Ok(bytes)
    }

    /// Build the validated `payload` of node `root` at `dest`.
    fn codec_build(
        &mut self,
        plan: &CodecPlan,
        image: &mut Extent,
        dest: Dest,
        root: CodecNodeId,
        payload: &[u8],
    ) -> Checked<()> {
        let mut input = Input {
            bytes: payload,
            at: 0,
        };
        let reread = |_| err(V::Malformed, "a validated codec payload reread short");
        let mut work = vec![(root, dest)];
        while let Some((id, dest)) = work.pop() {
            let node = self.plan_node(plan, id)?;
            let layout = node.layout();
            if self.record(layout)?.kind == LayoutKind::Flat
                || self.record(layout)?.kind == LayoutKind::Packed
                    && self.record(layout)?.align == 1
            {
                let bytes = self.codec_flat_build(plan, id, &mut input)?;
                self.codec_bytes(image, dest, &bytes);
                continue;
            }
            match node {
                CodecNode::FlatText { .. } | CodecNode::Framed { .. } => {
                    return fail(V::Malformed, "flat codec node off a flat layout");
                }
                CodecNode::Scalar(_) | CodecNode::Char(_) => {
                    let size = self.size(layout)?;
                    let bits = input.take(size).map_err(reread)?.to_vec();
                    self.codec_bytes(image, dest, &bits);
                }
                CodecNode::Text { .. } => {
                    let n = input.number(8).map_err(reread)? as u64;
                    let text = input.take(n).map_err(reread)?.to_vec();
                    self.codec_text(image, dest, node, &text)?;
                }
                CodecNode::Path(_) => {
                    if self.record(layout)?.kind != LayoutKind::Path {
                        return fail(V::TypeConfusion, "a codec path off its layout");
                    }
                    let n = input.number(8).map_err(reread)? as u64;
                    let text = std::str::from_utf8(input.take(n).map_err(reread)?)
                        .map_err(|_| err(V::Malformed, "a validated codec path reread"))?;
                    let id = self
                        .ops
                        .intern_path(text)
                        .ok_or_else(|| err(V::Unsupported, "the model cannot intern a Path"))?;
                    self.codec_bytes(image, dest, &id.to_le_bytes());
                }
                CodecNode::Fields { fields, .. } => {
                    for (ordinal, child) in fields.iter().enumerate().rev() {
                        let f = self.field_at(layout, ordinal as u32)?;
                        work.push((*child, dest.at(f.offset)));
                    }
                }
                CodecNode::Checked { value, .. } => work.push((*value, dest)),
                CodecNode::Variant { arms, .. } => {
                    let ordinal = input.number(4).map_err(reread)? as u32;
                    let (tag, offsets) = self.arm_fields(layout, ordinal)?;
                    self.codec_tag(image, dest, &tag, ordinal);
                    for (child, offset) in arms[ordinal as usize].iter().zip(offsets).rev() {
                        work.push((*child, dest.at(offset)));
                    }
                }
                CodecNode::Box { payload: p, .. } => {
                    let payload = self.block_payload(layout)?;
                    if matches!(self.record(payload)?.size, SizeSpec::Dynamic(_)) {
                        let bytes = self.codec_flat_build(plan, *p, &mut input)?;
                        let block = self.codec_block(
                            payload,
                            bytes.len() as u64,
                            self.record(payload)?.align,
                        )?;
                        self.codec_bytes(image, Dest::Heap(block), &bytes);
                        self.codec_cell(image, dest, block);
                    } else {
                        let size = self.size(payload)?;
                        let block = self.codec_block(payload, size, self.record(payload)?.align)?;
                        self.codec_cell(image, dest, block);
                        work.push((*p, Dest::Heap(block)));
                    }
                }
                CodecNode::Sequence { element, .. } => {
                    let count = input.number(8).map_err(reread)? as u64;
                    let p = self.block_payload(layout)?;
                    match self.record(p)?.kind {
                        LayoutKind::Sequence { .. } => {
                            let (block, start, stride) = self.codec_elements(layout, count)?;
                            self.codec_cell(image, dest, block);
                            for i in (0..count).rev() {
                                work.push((*element, Dest::Heap(block.at(start + i * stride))));
                            }
                        }
                        LayoutKind::Sequential { element: e } => {
                            let start =
                                hirsel_lir::layout::sequential_elements_offset(self.record(e)?)
                                    .map_err(|e| err(V::Malformed, format!("{e:?}")))?;
                            let mut bytes = Vec::new();
                            for _ in 0..count {
                                bytes.extend(self.codec_inline_build(plan, *element, &mut input)?);
                            }
                            let block = self.codec_block(
                                p,
                                start + bytes.len() as u64,
                                self.record(p)?.align,
                            )?;
                            self.codec_bytes(image, Dest::Heap(block), &count.to_le_bytes());
                            self.codec_bytes(
                                image,
                                Dest::Heap(block.at(8)),
                                &(bytes.len() as u64).to_le_bytes(),
                            );
                            self.codec_bytes(image, Dest::Heap(block.at(start)), &bytes);
                            self.codec_cell(image, dest, block);
                        }
                        LayoutKind::SoaColumns { element: e } => {
                            let CodecNode::Fields { fields, .. } =
                                self.plan_node(plan, *element)?
                            else {
                                return fail(V::Malformed, "codec column fields");
                            };
                            let (data, quantum, columns) = self.column_shape(e)?;
                            let capacity = hirsel_lir::layout::align_up(count, quantum)
                                .map_err(|e| err(V::Malformed, format!("{e:?}")))?;
                            let stride: u64 = columns.iter().map(|&(_, stride)| stride).sum();
                            let block = self.codec_block(
                                p,
                                data + capacity * stride,
                                self.record(p)?.align,
                            )?;
                            self.codec_bytes(image, Dest::Heap(block), &count.to_le_bytes());
                            self.codec_bytes(
                                image,
                                Dest::Heap(block.at(8)),
                                &capacity.to_le_bytes(),
                            );
                            self.codec_cell(image, dest, block);
                            self.clear_column_spare(block, e)?;
                            for i in (0..count).rev() {
                                for (child, (prefix, stride)) in fields.iter().zip(&columns).rev() {
                                    work.push((
                                        *child,
                                        Dest::Heap(block.at(data + capacity * prefix + i * stride)),
                                    ));
                                }
                            }
                        }
                        _ => return fail(V::Malformed, "codec sequence build payload"),
                    }
                }
                CodecNode::Repeat { element, .. } => {
                    let (e, count) = match self.record(layout)?.kind {
                        LayoutKind::Inline { element, count } => (element, count),
                        LayoutKind::Simd { element, lanes } => (element, u64::from(lanes)),
                        _ => return fail(V::TypeConfusion, "a codec repeat off its layout"),
                    };
                    let stride = self.size(e)?;
                    for i in (0..count).rev() {
                        work.push((*element, dest.at(i * stride)));
                    }
                }
                CodecNode::Map { .. } | CodecNode::Set { .. } => {
                    let count = input.number(8).map_err(reread)? as u64;
                    let container = self.block_payload(layout)?;
                    let size = self.size(container)?;
                    let block = self.codec_block(container, size, self.record(container)?.align)?;
                    let entries = self.field_at(container, CONTAINER_ENTRIES)?;
                    let index = self.field_at(container, CONTAINER_INDEX)?;
                    let (list, start, stride) = self.codec_elements(entries.layout, count)?;
                    // An empty index is a valid unindexed container.
                    let (unindexed, _, _) = self.codec_elements(index.layout, 0)?;
                    self.codec_cell(image, Dest::Heap(block.at(entries.offset)), list);
                    self.codec_cell(image, Dest::Heap(block.at(index.offset)), unindexed);
                    if matches!(node, CodecNode::Map { .. }) {
                        let hashes = self.field_at(container, 2)?;
                        let live = self.field_at(container, 3)?;
                        let (cache, _, _) = self.codec_elements(hashes.layout, 0)?;
                        self.codec_cell(image, Dest::Heap(block.at(hashes.offset)), cache);
                        self.codec_bytes(
                            image,
                            Dest::Heap(block.at(live.offset)),
                            &count.to_le_bytes(),
                        );
                    }
                    self.codec_cell(image, dest, block);
                    let pair = self.codec_sequence(entries.layout)?.1;
                    for i in (0..count).rev() {
                        let entry = block_entry(list, start, stride, i);
                        match node {
                            CodecNode::Map { key, value, .. } => {
                                let (tag, offsets) = self.arm_fields(pair, 1)?;
                                self.codec_tag(image, Dest::Heap(entry), &tag, 1);
                                let data =
                                    tag.variants.iter().find(|v| v.ordinal == 1).unwrap().fields[0]
                                        .layout;
                                let (k, v) = (self.field_at(data, 0)?, self.field_at(data, 1)?);
                                work.push((*value, Dest::Heap(entry.at(offsets[0] + v.offset))));
                                work.push((*key, Dest::Heap(entry.at(offsets[0] + k.offset))));
                            }
                            CodecNode::Set { element, .. } => {
                                work.push((*element, Dest::Heap(entry)))
                            }
                            _ => unreachable!(),
                        }
                    }
                }
            }
        }
        if input.at != payload.len() {
            return fail(V::Malformed, "a validated codec payload rebuilt short");
        }
        Ok(())
    }
}

fn block_entry(list: Ptr, start: u64, stride: u64, i: u64) -> Ptr {
    list.at(start + i * stride)
}
