//! Value operations over an already admitted, validated physical access program.
//! These walks allocate only ordinary runtime scratch; they do not encode a wire
//! value, invoke checks, consume operands, or debit a language/arena budget.
use super::{Node, Program, Scalar, flat_extent, pointer, word};
use crate::{TaskCtx, alloc::payload, fault, kernels::siphash::SipHasher13};
use std::cmp::Ordering;

#[derive(Clone, Copy)]
enum Location {
    Address(*const u8),
    Row {
        columns: usize,
        base: *const u8,
        capacity: u64,
        row: u64,
    },
}
#[derive(Clone, Copy)]
struct Projection {
    node: usize,
    location: Location,
}
impl Projection {
    fn at(node: usize, at: *const u8) -> Self {
        Self {
            node,
            location: Location::Address(at),
        }
    }
}
enum View<'a> {
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
unsafe fn view<'a>(p: &Program, mut projection: Projection) -> View<'a> {
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
                    return View::Bytes(super::text_bytes(text, at));
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
unsafe fn bits(at: *const u8, width: usize) -> u128 {
    let mut bytes = [0u8; 16];
    unsafe {
        std::ptr::copy_nonoverlapping(at, bytes.as_mut_ptr(), width);
    }
    u128::from_le_bytes(bytes)
}
unsafe fn scalar_order(kind: Scalar, code: u64, a: *const u8, b: *const u8) -> Option<Ordering> {
    unsafe {
        Some(match kind {
            Scalar::Bool => (a.read() != 0).cmp(&(b.read() != 0)),
            Scalar::Char => bits(a, 4).cmp(&bits(b, 4)),
            Scalar::Integer(width) => {
                let (a, b) = (bits(a, width), bits(b, width));
                if (6..=10).contains(&code) {
                    let shift = 128 - width * 8;
                    (((a << shift) as i128) >> shift).cmp(&(((b << shift) as i128) >> shift))
                } else {
                    a.cmp(&b)
                }
            }
            Scalar::F32 => {
                return f32::from_bits(bits(a, 4) as u32)
                    .partial_cmp(&f32::from_bits(bits(b, 4) as u32));
            }
            Scalar::F64 => {
                return f64::from_bits(bits(a, 8) as u64)
                    .partial_cmp(&f64::from_bits(bits(b, 8) as u64));
            }
        })
    }
}
unsafe fn canonical_set(p: &Program, ctx: *mut TaskCtx, children: &mut [Projection]) {
    children.sort_by(|a, b| unsafe {
        order(p, ctx, *a, *b).unwrap_or_else(|| fault("unordered structural Set key"))
    });
}
unsafe fn order(p: &Program, ctx: *mut TaskCtx, a: Projection, b: Projection) -> Option<Ordering> {
    enum Item {
        Pair(Projection, Projection),
        Length(u64, u64),
    }
    let mut work = vec![Item::Pair(a, b)];
    while let Some(item) = work.pop() {
        let result = match item {
            Item::Length(a, b) => Some(a.cmp(&b)),
            Item::Pair(a, b) => unsafe {
                match (view(p, a), view(p, b)) {
                    (View::Scalar { kind, code, at: a }, View::Scalar { at: b, .. }) => {
                        scalar_order(kind, code, a, b)
                    }
                    (View::Bytes(a), View::Bytes(b)) => Some(a.cmp(b)),
                    (View::Path(a), View::Path(b)) => {
                        Some(crate::io::hirsel_rt_v2_path_compare(ctx, a, b).cmp(&0))
                    }
                    (
                        View::Children {
                            ordinal: a,
                            count: an,
                            set,
                            children: mut ac,
                        },
                        View::Children {
                            ordinal: b,
                            count: bn,
                            children: mut bc,
                            ..
                        },
                    ) => {
                        if a != b {
                            return Some(a.cmp(&b));
                        }
                        if set {
                            canonical_set(p, ctx, &mut ac);
                            canonical_set(p, ctx, &mut bc);
                        }
                        if let (Some(a), Some(b)) = (an, bn) {
                            work.push(Item::Length(a, b));
                        }
                        for (a, b) in ac.into_iter().zip(bc).rev() {
                            work.push(Item::Pair(a, b));
                        }
                        Some(Ordering::Equal)
                    }
                    _ => fault("structural access view mismatch"),
                }
            },
        };
        if result != Some(Ordering::Equal) {
            return result;
        }
    }
    Some(Ordering::Equal)
}

pub(crate) unsafe fn compare(words: &[u64], ctx: *mut TaskCtx, a: *const u8, b: *const u8) -> u8 {
    let p = Program::parse_access(words);
    match unsafe {
        order(
            &p,
            ctx,
            Projection::at(p.root, a),
            Projection::at(p.root, b),
        )
    } {
        Some(Ordering::Equal) => 0,
        Some(Ordering::Less) => 1,
        Some(Ordering::Greater) => 2,
        None => 3,
    }
}

pub(crate) unsafe fn hash(words: &[u64], ctx: *mut TaskCtx, at: *const u8, seed: u64) -> u64 {
    let p = Program::parse_access(words);
    let mut hash = SipHasher13::new(seed, crate::HASH_KEY_1);
    let mut work = vec![Projection::at(p.root, at)];
    while let Some(item) = work.pop() {
        unsafe {
            match view(&p, item) {
                View::Scalar { kind, at, .. } => match kind {
                    Scalar::Bool => hash.write(&[u8::from(at.read() != 0)]),
                    Scalar::Integer(width) => hash.write(std::slice::from_raw_parts(at, width)),
                    Scalar::Char => hash.write(std::slice::from_raw_parts(at, 4)),
                    Scalar::F32 => {
                        let n = f32::from_bits(bits(at, 4) as u32);
                        hash.write(&if n == 0.0 { 0 } else { n.to_bits() }.to_le_bytes());
                    }
                    Scalar::F64 => {
                        let n = f64::from_bits(bits(at, 8) as u64);
                        hash.write(&if n == 0.0 { 0 } else { n.to_bits() }.to_le_bytes());
                    }
                },
                View::Bytes(bytes) => {
                    hash.write(&(bytes.len() as u64).to_le_bytes());
                    hash.write(bytes);
                }
                View::Path(id) => {
                    let names = crate::paths::names(ctx);
                    let text = names.path_text(id);
                    hash.write(&(text.len() as u64).to_le_bytes());
                    hash.write(text.as_bytes());
                }
                View::Children {
                    ordinal,
                    count,
                    set,
                    mut children,
                } => {
                    if let Some(tag) = ordinal {
                        hash.write(&tag.to_le_bytes());
                    }
                    if let Some(count) = count {
                        hash.write(&count.to_le_bytes());
                    }
                    if set {
                        canonical_set(&p, ctx, &mut children);
                    }
                    work.extend(children.into_iter().rev());
                }
            }
        }
    }
    hash.finish()
}
