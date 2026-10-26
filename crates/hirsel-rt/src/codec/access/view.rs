//! Value operations over an already admitted, validated physical access program.
//! The owning access API uses these projections. Native order/hash stream the
//! emitted words directly, without constructing this owning view.
use super::super::{Node, Program, Scalar, flat_extent, pointer, word};
use crate::{alloc::payload, fault};

#[derive(Clone, Copy)]
pub(super) enum Location {
    Address(*const u8),
    Row {
        columns: usize,
        base: *const u8,
        capacity: u64,
        row: u64,
    },
}
#[derive(Clone, Copy)]
pub(super) struct Projection {
    pub(super) node: usize,
    pub(super) location: Location,
}
impl Projection {
    pub(super) fn at(node: usize, at: *const u8) -> Self {
        Self {
            node,
            location: Location::Address(at),
        }
    }
}
pub(super) enum View<'a> {
    Scalar {
        kind: Scalar,
        code: u64,
        at: *const u8,
    },
    Bytes(&'a [u8]),
    Path(u64),
    Children {
        ordinal: Option<u32>,
        count: Option<u64>,
        set: bool,
        children: Vec<Projection>,
    },
}

// SAFETY: callers supply live admitted operands of the program's validated root.
// Every projected address below follows that same program; all reads are borrowed.
pub(super) unsafe fn view<'a>(p: &Program, mut projection: Projection) -> View<'a> {
    unsafe {
        loop {
            let id = projection.node;
            let node = &p.nodes[id];
            if let Node::Checked { value, .. } = node {
                projection.node = *value;
                continue;
            }
            if let Location::Row {
                columns,
                base,
                capacity,
                row,
            } = projection.location
            {
                let Node::Columns { data, columns, .. } = &p.nodes[columns] else {
                    unreachable!()
                };
                if !matches!(node, Node::Fields(_)) {
                    fault("structural virtual row without fields");
                }
                let children = columns
                    .iter()
                    .map(|&(prefix, stride, child)| {
                        Projection::at(
                            child,
                            base.add((*data + capacity * prefix + row * stride) as usize),
                        )
                    })
                    .collect();
                return View::Children {
                    ordinal: None,
                    count: None,
                    set: false,
                    children,
                };
            }
            let Location::Address(at) = projection.location else {
                unreachable!()
            };
            let mut ordinal = None;
            let mut count = None;
            let mut set = false;
            let children = match node {
                Node::Scalar(kind) => {
                    return View::Scalar {
                        kind: *kind,
                        code: p.scalar_codes[id],
                        at,
                    };
                }
                Node::Path => return View::Path(word(at)),
                Node::Text(text) => {
                    return View::Bytes(super::super::text_bytes(text, at));
                }
                Node::FlatText { .. } => {
                    return View::Bytes(std::slice::from_raw_parts(at.add(8), word(at) as usize));
                }
                Node::Framed(value) => {
                    projection = Projection::at(*value, at.add(8));
                    continue;
                }
                Node::Checked { .. } => unreachable!(),
                Node::Box { payload: value, .. } => {
                    projection = Projection::at(*value, payload(pointer(at)));
                    continue;
                }
                Node::Fields(fields) => fields
                    .iter()
                    .map(|&(offset, child)| Projection::at(child, at.add(offset as usize)))
                    .collect(),
                Node::Variant {
                    niche,
                    tag,
                    nullary,
                    arms,
                    ..
                } => {
                    let tag = if *niche {
                        if word(at.add(*tag as usize)) == 0 {
                            *nullary
                        } else {
                            1 - *nullary
                        }
                    } else {
                        at.add(*tag as usize).cast::<u32>().read_unaligned()
                    };
                    ordinal = Some(tag);
                    arms.get(tag as usize)
                        .unwrap_or_else(|| fault("structural variant ordinal"))
                        .iter()
                        .map(|&(offset, child)| Projection::at(child, at.add(offset as usize)))
                        .collect()
                }
                Node::FlatFields(fields) => flat_children(p, fields, at),
                Node::FlatVariant(arms) => {
                    let tag = at.cast::<u32>().read_unaligned();
                    ordinal = Some(tag);
                    flat_children(
                        p,
                        arms.get(tag as usize)
                            .unwrap_or_else(|| fault("structural flat ordinal")),
                        at.add(4),
                    )
                }
                Node::Repeat {
                    count: n,
                    stride,
                    element,
                } => {
                    count = Some(*n);
                    (0..*n)
                        .map(|i| Projection::at(*element, at.add((i * stride) as usize)))
                        .collect()
                }
                Node::Sequence { elements, element } => {
                    let base = payload(pointer(at));
                    let n = word(base);
                    count = Some(n);
                    (0..n)
                        .map(|i| {
                            Projection::at(
                                *element,
                                base.add((elements.offset + i * elements.stride) as usize),
                            )
                        })
                        .collect()
                }
                Node::FlatSequence {
                    offset,
                    stride,
                    element,
                } => {
                    let n = if *offset == 16 {
                        word(at.add(8))
                    } else {
                        word(at) / stride
                    };
                    count = Some(n);
                    sequential_children(p, *element, at.add(*offset as usize), n)
                }
                Node::Sequential {
                    offset, element, ..
                } => {
                    let base = payload(pointer(at));
                    let n = word(base);
                    count = Some(n);
                    sequential_children(p, *element, base.add(*offset as usize), n)
                }
                Node::Columns { element, .. } => {
                    let base = payload(pointer(at));
                    let n = word(base);
                    let capacity = word(base.add(8));
                    count = Some(n);
                    (0..n)
                        .map(|row| Projection {
                            node: *element,
                            location: Location::Row {
                                columns: id,
                                base,
                                capacity,
                                row,
                            },
                        })
                        .collect()
                }
                Node::Container(container) => {
                    set = container.set;
                    let base = payload(pointer(at));
                    let entries = payload(pointer(base.add(container.entries_at as usize)));
                    let mut n = 0;
                    let mut children = vec![];
                    for i in 0..word(entries) {
                        let entry = entries.add(
                            (container.entries.offset + i * container.entries.stride) as usize,
                        );
                        if !container.map.expect("structural map storage").live(entry) {
                            continue;
                        }
                        n += 1;
                        children.push(Projection::at(
                            container.key,
                            entry.add(container.key_at as usize),
                        ));
                        if let Some(value) = container.value {
                            children.push(Projection::at(
                                value,
                                entry.add(container.value_at as usize),
                            ));
                        }
                    }
                    count = Some(n);
                    children
                }
            };
            return View::Children {
                ordinal,
                count,
                set,
                children,
            };
        }
    }
}
unsafe fn flat_children(p: &Program, fields: &[usize], mut at: *const u8) -> Vec<Projection> {
    let mut children = Vec::with_capacity(fields.len());
    for &child in fields {
        children.push(Projection::at(child, at));
        at = at.wrapping_add(unsafe { flat_extent(p, child, at) } as usize);
    }
    children
}
unsafe fn sequential_children(
    p: &Program,
    element: usize,
    mut at: *const u8,
    count: u64,
) -> Vec<Projection> {
    let mut children = Vec::new();
    for _ in 0..count {
        children.push(Projection::at(element, at));
        at = at.wrapping_add(unsafe { flat_extent(p, element, at) } as usize);
    }
    children
}
