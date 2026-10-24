//! H-only physical projections from checked layouts and ledger cells.
//!
//! Factored from logical-depth-impl-31's structural walker at 0bad1fa3.
//! A view expands one node; clients choose Shared traversal and maintain their
//! own worklists, depth accounting, semantic inventory and ownership state.
use super::compare::At;
use super::*;
use hirsel_lir::codec::{CodecNode as Node, CodecNodeId as NodeId};
use hirsel_lir::structural_access::AccessPlan;

/// A column row has no contiguous address. Preserve it through representation
/// wrappers until Fields projects each column at its independently read capacity.
#[derive(Clone, Copy)]
pub(super) enum Location<'e> {
    Stored(At<'e>),
    Columns { row: ColumnRow, element: LayoutId },
}

#[derive(Clone, Copy)]
pub(super) struct Projection<'e> {
    pub node: NodeId,
    pub location: Location<'e>,
}

#[derive(Clone, Copy, Eq, PartialEq)]
pub(super) enum SharedTraversal {
    Stop,
    Follow,
}

#[derive(Clone, Copy)]
pub(super) struct ByteSpan<'e> {
    pub at: At<'e>,
    pub len: u64,
}

pub(super) struct View<'p, 'e> {
    pub logical: u32,
    pub node: &'p Node,
    pub shared: bool,
    pub ordinal: Option<u32>,
    /// Element count, or live entry count for Map/Set. Children of a Map are
    /// key/value pairs in live storage order; tombstones have no children.
    pub count: Option<u64>,
    pub bytes: Option<ByteSpan<'e>>,
    pub children: Vec<Projection<'e>>,
}

fn add(a: u64, b: u64) -> Checked<u64> {
    a.checked_add(b)
        .ok_or_else(|| err(V::OutOfExtent, "projection offset overflow"))
}
fn mul(a: u64, b: u64) -> Checked<u64> {
    a.checked_mul(b)
        .ok_or_else(|| err(V::OutOfExtent, "projection stride overflow"))
}
fn offset(at: At<'_>, delta: u64) -> Checked<At<'_>> {
    Ok(match at {
        At::Image(image, base) => At::Image(image, add(base, delta)?),
        At::Heap(ptr) => {
            add(ptr.offset, delta)?;
            At::Heap(ptr.at(delta))
        }
    })
}
fn stored(location: Location<'_>) -> Checked<At<'_>> {
    match location {
        Location::Stored(at) => Ok(at),
        Location::Columns { .. } => fail(V::Malformed, "non-record column projection"),
    }
}

impl<S: Stage> Machine<'_, S> {
    pub(super) fn projection_node<'p>(
        &self,
        plan: &'p AccessPlan,
        id: NodeId,
    ) -> Checked<&'p Node> {
        plan.nodes
            .get(id.index())
            .ok_or_else(|| err(V::Malformed, "projection node outside plan"))
    }

    pub(super) fn projection_image(&self, plan: &AccessPlan, value: &Val) -> Checked<Extent> {
        let layout = self.projection_node(plan, plan.root)?.layout();
        Ok(match value {
            Val::Image(image) => image.clone(),
            Val::Unit => Extent::new(0),
            Val::Bits { bits, .. } => {
                let size = self.size(layout)?;
                let bytes = bits.to_le_bytes();
                let part = bytes
                    .get(..size as usize)
                    .ok_or_else(|| err(V::TypeConfusion, "projection scalar root size"))?;
                let mut image = Extent::new(size);
                image.write_bytes(0, part);
                image
            }
            Val::Ptr(ptr) => {
                let mut image = Extent::new(self.word);
                image.put_cell(
                    0,
                    self.word,
                    Cell::Data {
                        ptr: *ptr,
                        counted: false,
                        borrow: None,
                    },
                );
                image
            }
            _ => return fail(V::TypeConfusion, "projection root operand"),
        })
    }

    fn projection_field(
        &self,
        layout: LayoutId,
        ordinal: usize,
    ) -> Checked<hirsel_lir::layout_types::FieldLayout> {
        self.record(layout)?
            .fields
            .iter()
            .find(|f| f.ordinal as usize == ordinal)
            .cloned()
            .ok_or_else(|| err(V::Malformed, "projection field"))
    }

    fn projection_payload(&self, layout: LayoutId) -> Checked<LayoutId> {
        match self.record(layout)?.kind {
            LayoutKind::Block { payload }
            | LayoutKind::Box { payload }
            | LayoutKind::Shared { payload } => Ok(payload),
            _ => fail(V::Malformed, "projection block payload"),
        }
    }

    fn projection_offsets(
        &self,
        layout: LayoutId,
        fields: &[hirsel_lir::layout_types::FieldLayout],
        at: At<'_>,
    ) -> Checked<Vec<u64>> {
        let row = self.record(layout)?;
        let packed =
            row.kind == LayoutKind::Flat || row.kind == LayoutKind::Packed && row.align == 1;
        let mut extra = 0;
        fields
            .iter()
            .map(|field| {
                let delta = add(field.offset, extra)?;
                if packed && matches!(self.record(field.layout)?.size, SizeSpec::Dynamic(_)) {
                    extra = add(extra, self.bits_at(offset(at, delta)?, 8)? as u64)?;
                }
                Ok(delta)
            })
            .collect()
    }

    pub(super) fn projection_bytes<'e>(&'e self, span: ByteSpan<'e>) -> Checked<&'e [u8]> {
        let (image, base) = self.extent_at(span.at)?;
        image.check_range(base, span.len)?;
        if !image.all_init(base, span.len) {
            return fail(V::UninitializedRead, "projection byte span");
        }
        Ok(image.read_bytes(base, span.len))
    }

    /// One checked view, with no logical read/depth limit and no relocation.
    /// Stop returns a Shared boundary without dereferencing the root cell.
    pub(super) fn projection_view<'p, 'e>(
        &self,
        plan: &'p AccessPlan,
        projection: Projection<'e>,
        shared: SharedTraversal,
    ) -> Checked<View<'p, 'e>> {
        let node = self.projection_node(plan, projection.node)?;
        let layout = node.layout();
        let logical = *plan
            .logical_nodes
            .get(projection.node.index())
            .filter(|&&id| (id as usize) < plan.graph.nodes.len())
            .ok_or_else(|| err(V::Malformed, "projection logical correspondence"))?;
        let mut view = View {
            logical,
            node,
            shared: matches!(self.record(layout)?.kind, LayoutKind::Shared { .. }),
            ordinal: None,
            count: None,
            bytes: None,
            children: vec![],
        };
        if view.shared && shared == SharedTraversal::Stop {
            return Ok(view);
        }
        let child = |node, location| Projection { node, location };
        let at_child = |node, at| child(node, Location::Stored(at));
        if let Location::Columns { row, element } = projection.location {
            match node {
                Node::Checked { value, .. } => {
                    view.children.push(child(*value, projection.location));
                }
                Node::Fields { fields, .. } => {
                    let (data, _, columns) = self.column_shape(element)?;
                    if fields.len() != columns.len() {
                        return fail(V::Malformed, "projection column field count");
                    }
                    for (&id, (prefix, stride)) in fields.iter().zip(columns) {
                        let delta = add(
                            add(data, mul(row.capacity, prefix)?)?,
                            mul(row.index, stride)?,
                        )?;
                        let at = offset(At::Heap(row.base), delta)?;
                        let (image, base) = self.extent_at(at)?;
                        image.check_range(base, stride)?;
                        view.children.push(at_child(id, at));
                    }
                }
                _ => return fail(V::Malformed, "projection virtual row kind"),
            }
            return Ok(view);
        }
        let at = stored(projection.location)?;
        match node {
            Node::Scalar(_) | Node::Char(_) | Node::Path(_) => (),
            Node::Text { .. } => {
                let owner = self.projection_field(layout, 0)?;
                let start = self.projection_field(layout, 1)?;
                let len = self.projection_field(layout, 2)?;
                let owner = self.ptr_at(offset(at, owner.offset)?)?;
                let start = self.bits_at(offset(at, start.offset)?, 8)? as u64;
                let len = self.bits_at(offset(at, len.offset)?, 8)? as u64;
                let data = offset(At::Heap(owner), add(8, start)?)?;
                let span = ByteSpan { at: data, len };
                self.projection_bytes(span)?;
                view.bytes = Some(span);
            }
            Node::FlatText { .. } => {
                let span = ByteSpan {
                    at: offset(at, 8)?,
                    len: self.bits_at(at, 8)? as u64,
                };
                self.projection_bytes(span)?;
                view.bytes = Some(span);
            }
            Node::Framed { value, .. } => view.children.push(at_child(*value, offset(at, 8)?)),
            Node::Checked { value, .. } => view.children.push(at_child(*value, at)),
            Node::Fields { fields, .. } => {
                let rows = (0..fields.len())
                    .map(|i| self.projection_field(layout, i))
                    .collect::<Checked<Vec<_>>>()?;
                for (&id, delta) in fields
                    .iter()
                    .zip(self.projection_offsets(layout, &rows, at)?)
                {
                    view.children.push(at_child(id, offset(at, delta)?));
                }
            }
            Node::Variant { arms, .. } => {
                let tag = self
                    .record(layout)?
                    .tag
                    .as_ref()
                    .ok_or_else(|| err(V::Malformed, "projection variant tag"))?;
                let ordinal = self.tag_at(offset(at, tag.offset)?, tag.form)?;
                let ids = arms
                    .get(ordinal as usize)
                    .ok_or_else(|| err(V::TypeConfusion, "projection variant ordinal"))?;
                let arm = tag
                    .variants
                    .iter()
                    .find(|v| v.ordinal == ordinal)
                    .ok_or_else(|| err(V::Malformed, "projection variant arm"))?;
                if ids.len() != arm.fields.len() {
                    return fail(V::Malformed, "projection variant field count");
                }
                view.ordinal = Some(ordinal);
                for (&id, delta) in
                    ids.iter()
                        .zip(self.projection_offsets(layout, &arm.fields, at)?)
                {
                    view.children.push(at_child(id, offset(at, delta)?));
                }
            }
            Node::Box { payload, .. } => {
                view.children
                    .push(at_child(*payload, At::Heap(self.ptr_at(at)?)));
            }
            Node::Repeat { element, .. } => {
                let (layout, count) = match self.record(layout)?.kind {
                    LayoutKind::Inline { element, count } => (element, count),
                    LayoutKind::Simd { element, lanes } => (element, u64::from(lanes)),
                    _ => return fail(V::Malformed, "projection repeated storage"),
                };
                let stride = self.size(layout)?;
                let (image, base) = self.extent_at(at)?;
                image.check_range(base, mul(count, stride)?)?;
                view.count = Some(count);
                for i in 0..count {
                    view.children
                        .push(at_child(*element, offset(at, mul(i, stride)?)?));
                }
            }
            Node::Sequence { element, .. } => {
                let child_layout = self.projection_node(plan, *element)?.layout();
                if self.record(layout)?.kind == LayoutKind::Flat {
                    let (start, _) = self
                        .flat_prefixed(layout)?
                        .ok_or_else(|| err(V::Malformed, "projection flat sequence"))?;
                    let At::Heap(block) = at else {
                        return fail(V::TypeConfusion, "flat sequence outside block");
                    };
                    let count = self.flat_count(layout, block)?;
                    view.count = Some(count);
                    let mut cursor = start;
                    for _ in 0..count {
                        let loc = offset(at, cursor)?;
                        view.children.push(at_child(*element, loc));
                        let At::Heap(ptr) = loc else { unreachable!() };
                        cursor = add(cursor, self.extent_of(child_layout, ptr)?)?;
                    }
                    let (image, base) = self.extent_at(at)?;
                    image.check_range(base, cursor)?;
                    return Ok(view);
                }
                let payload = self.projection_payload(layout)?;
                let block = self.ptr_at(at)?;
                let count = self.bits_at(At::Heap(block), 8)? as u64;
                view.count = Some(count);
                match self.record(payload)?.kind {
                    LayoutKind::Sequence { element: e } => {
                        let (start, stride) = self.sequence_shape(e)?;
                        let (image, base) = self.extent_at(At::Heap(block))?;
                        image.check_range(base, add(start, mul(count, stride)?)?)?;
                        for i in 0..count {
                            view.children.push(at_child(
                                *element,
                                offset(At::Heap(block), add(start, mul(i, stride)?)?)?,
                            ));
                        }
                    }
                    LayoutKind::Sequential { element: e } => {
                        let mut cursor =
                            hirsel_lir::layout::sequential_elements_offset(self.record(e)?)
                                .map_err(|e| err(V::Malformed, format!("{e:?}")))?;
                        for _ in 0..count {
                            let at = offset(At::Heap(block), cursor)?;
                            view.children.push(at_child(*element, at));
                            let At::Heap(ptr) = at else { unreachable!() };
                            cursor = add(cursor, self.extent_of(e, ptr)?)?;
                        }
                        let (image, base) = self.extent_at(At::Heap(block))?;
                        image.check_range(base, cursor)?;
                    }
                    LayoutKind::SoaColumns { element: e } => {
                        let capacity = self.columns_capacity(block, e)?;
                        if count > capacity {
                            return fail(
                                V::OutOfExtent,
                                "projection column count exceeds capacity",
                            );
                        }
                        for index in 0..count {
                            view.children.push(child(
                                *element,
                                Location::Columns {
                                    row: ColumnRow {
                                        base: block,
                                        capacity,
                                        index,
                                    },
                                    element: e,
                                },
                            ));
                        }
                    }
                    _ => return fail(V::Malformed, "projection sequence storage"),
                }
            }
            Node::Map { .. } | Node::Set { .. } => {
                let container = self.projection_payload(layout)?;
                let entries = self.projection_field(container, 0)?;
                let storage = self.projection_payload(entries.layout)?;
                let LayoutKind::Sequence { element: entry } = self.record(storage)?.kind else {
                    return fail(V::Malformed, "projection map entries");
                };
                let (start, stride) = self.sequence_shape(entry)?;
                let block = self.ptr_at(offset(At::Heap(self.ptr_at(at)?), entries.offset)?)?;
                let count = self.bits_at(At::Heap(block), 8)? as u64;
                let (image, base) = self.extent_at(At::Heap(block))?;
                image.check_range(base, add(start, mul(count, stride)?)?)?;
                let tag = self
                    .record(entry)?
                    .tag
                    .as_ref()
                    .ok_or_else(|| err(V::Malformed, "projection map tag"))?;
                let data = tag
                    .variants
                    .iter()
                    .find(|v| v.ordinal == 1)
                    .and_then(|v| v.fields.first())
                    .ok_or_else(|| err(V::Malformed, "projection map pair"))?;
                let key = self.projection_field(data.layout, 0)?;
                let (key_id, value) = match node {
                    Node::Map { key, value, .. } => {
                        (*key, Some((*value, self.projection_field(data.layout, 1)?)))
                    }
                    Node::Set { element, .. } => (*element, None),
                    _ => unreachable!(),
                };
                let mut live = 0;
                for i in 0..count {
                    let entry = offset(At::Heap(block), add(start, mul(i, stride)?)?)?;
                    let ordinal = self.tag_at(offset(entry, tag.offset)?, tag.form)?;
                    if !tag.variants.iter().any(|v| v.ordinal == ordinal) {
                        return fail(V::TypeConfusion, "projection map entry ordinal");
                    }
                    if ordinal != 1 {
                        continue;
                    }
                    live += 1;
                    view.children.push(at_child(
                        key_id,
                        offset(entry, add(data.offset, key.offset)?)?,
                    ));
                    if let Some((id, field)) = &value {
                        view.children.push(at_child(
                            *id,
                            offset(entry, add(data.offset, field.offset)?)?,
                        ));
                    }
                }
                view.count = Some(live);
            }
        }
        Ok(view)
    }
}

#[cfg(test)]
mod tests;
