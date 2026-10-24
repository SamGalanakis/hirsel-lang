//! Access-only MEM-3 preflight. Physical addressing follows the codec's
//! layout program; no wire schema, serialization budget or output is involved.
use super::{Container, Node, Program, flat_extent, pointer, word};
use crate::{alloc::payload, fault};

#[derive(Clone, Copy)]
enum Job {
    Visit {
        node: usize,
        at: *const u8,
        depth: u64,
    },
    Elements {
        node: usize,
        at: *const u8,
        depth: u64,
        count: u64,
        stride: u64,
    },
    Sequential {
        node: usize,
        at: *const u8,
        depth: u64,
        count: u64,
    },
    Entries {
        container: Container,
        at: *const u8,
        depth: u64,
        count: u64,
    },
    Rows {
        node: usize,
        at: *const u8,
        depth: u64,
        row: u64,
        count: u64,
        capacity: u64,
    },
}

const LIMIT: u64 = hirsel_core_contract::depth::STRUCTURAL_LIMIT;

/// Validate the virtual row, including distinct wrappers even when it has no
/// columns. The physical program retains the represented record's node ID.
fn column_field_depth(
    program: &Program,
    edges: &[u8],
    mut node: usize,
    mut depth: u64,
) -> Option<u64> {
    for _ in 0..=program.nodes.len() {
        if depth > LIMIT {
            return None;
        }
        let next = depth + u64::from(edges[node]);
        match &program.nodes[node] {
            Node::Checked { value, .. } => {
                node = *value;
                depth = next;
            }
            Node::Fields(_) => return Some(next),
            _ => fault("structural columns without represented fields"),
        }
    }
    fault("structural column representation cycle")
}

/// The pointers and program come from independently validated emitted access
/// metadata and a live operand. A false result is the sole language failure;
/// physical allocation failure is left to the ordinary environmental boundary.
pub(crate) unsafe fn preflight(words: &[u64], edges: &[u8], value: *const u8) -> bool {
    let program = Program::parse_access(words);
    if edges.len() != program.nodes.len() || edges.iter().any(|&n| n > 1) {
        fault("malformed structural access edge table");
    }
    let mut work = vec![Job::Visit {
        node: program.root,
        at: value,
        depth: 0,
    }];
    while let Some(job) = work.pop() {
        // SAFETY: every work item is a projection of the live operand by the
        // validated physical access program. No value is consumed or changed.
        unsafe {
            match job {
                Job::Elements {
                    node,
                    at,
                    depth,
                    count,
                    stride,
                } => {
                    if count == 0 {
                        continue;
                    }
                    work.push(Job::Elements {
                        node,
                        at: at.wrapping_add(stride as usize),
                        depth,
                        count: count - 1,
                        stride,
                    });
                    work.push(Job::Visit { node, at, depth });
                }
                Job::Sequential {
                    node,
                    at,
                    depth,
                    count,
                } => {
                    if count == 0 {
                        continue;
                    }
                    let extent = flat_extent(&program, node, at);
                    work.push(Job::Sequential {
                        node,
                        at: at.wrapping_add(extent as usize),
                        depth,
                        count: count - 1,
                    });
                    work.push(Job::Visit { node, at, depth });
                }
                Job::Entries {
                    container,
                    at,
                    depth,
                    count,
                } => {
                    if count == 0 {
                        continue;
                    }
                    work.push(Job::Entries {
                        container,
                        at: at.wrapping_add(container.entries.stride as usize),
                        depth,
                        count: count - 1,
                    });
                    if !container.map.expect("map physical storage").live(at) {
                        continue;
                    }
                    if let Some(node) = container.value {
                        work.push(Job::Visit {
                            node,
                            at: at.add(container.value_at as usize),
                            depth,
                        });
                    }
                    work.push(Job::Visit {
                        node: container.key,
                        at: at.add(container.key_at as usize),
                        depth,
                    });
                }
                Job::Rows {
                    node,
                    at,
                    depth,
                    row,
                    count,
                    capacity,
                } => {
                    if row == count {
                        continue;
                    }
                    let Node::Columns { data, columns, .. } = &program.nodes[node] else {
                        unreachable!()
                    };
                    work.push(Job::Rows {
                        node,
                        at,
                        depth,
                        row: row + 1,
                        count,
                        capacity,
                    });
                    for &(prefix, stride, child) in columns.iter().rev() {
                        work.push(Job::Visit {
                            node: child,
                            at: at.add((*data + capacity * prefix + row * stride) as usize),
                            depth,
                        });
                    }
                }
                Job::Visit { node, at, depth } => {
                    if depth > LIMIT {
                        return false;
                    }
                    let edge = *edges
                        .get(node)
                        .unwrap_or_else(|| fault("structural access reaches output node"));
                    let child_depth = depth + u64::from(edge);
                    match &program.nodes[node] {
                        Node::Scalar(_) | Node::Text(_) | Node::FlatText { .. } | Node::Path => (),
                        Node::Checked { value, .. } => work.push(Job::Visit {
                            node: *value,
                            at,
                            depth: child_depth,
                        }),
                        Node::Framed(child) => work.push(Job::Visit {
                            node: *child,
                            at: at.add(8),
                            depth: child_depth,
                        }),
                        Node::Fields(fields) => {
                            for &(offset, child) in fields.iter().rev() {
                                work.push(Job::Visit {
                                    node: child,
                                    at: at.add(offset as usize),
                                    depth: child_depth,
                                });
                            }
                        }
                        Node::Variant {
                            niche,
                            tag,
                            nullary,
                            arms,
                            ..
                        } => {
                            let ordinal = if *niche {
                                if word(at.add(*tag as usize)) == 0 {
                                    *nullary
                                } else {
                                    1 - *nullary
                                }
                            } else {
                                at.add(*tag as usize).cast::<u32>().read_unaligned()
                            };
                            let fields = arms
                                .get(ordinal as usize)
                                .unwrap_or_else(|| fault("structural variant tag"));
                            for &(offset, child) in fields.iter().rev() {
                                work.push(Job::Visit {
                                    node: child,
                                    at: at.add(offset as usize),
                                    depth: child_depth,
                                });
                            }
                        }
                        Node::Box { payload: child, .. } => work.push(Job::Visit {
                            node: *child,
                            at: payload(pointer(at)),
                            depth: child_depth,
                        }),
                        Node::Sequence { elements, element } => {
                            let at = payload(pointer(at));
                            let count = word(at);
                            work.push(Job::Elements {
                                node: *element,
                                at: at.add(elements.offset as usize),
                                depth: child_depth,
                                count,
                                stride: elements.stride,
                            });
                        }
                        Node::Repeat {
                            count,
                            stride,
                            element,
                        } => work.push(Job::Elements {
                            node: *element,
                            at,
                            depth: child_depth,
                            count: *count,
                            stride: *stride,
                        }),
                        Node::FlatFields(fields) => {
                            let mut cursor = at;
                            let mut children = Vec::with_capacity(fields.len());
                            for &child in fields {
                                children.push(Job::Visit {
                                    node: child,
                                    at: cursor,
                                    depth: child_depth,
                                });
                                cursor = cursor
                                    .wrapping_add(flat_extent(&program, child, cursor) as usize);
                            }
                            work.extend(children.into_iter().rev());
                        }
                        Node::FlatVariant(arms) => {
                            let ordinal = at.cast::<u32>().read_unaligned();
                            let fields = arms
                                .get(ordinal as usize)
                                .unwrap_or_else(|| fault("structural flat variant tag"));
                            let mut cursor = at.add(4);
                            let mut children = Vec::with_capacity(fields.len());
                            for &child in fields {
                                children.push(Job::Visit {
                                    node: child,
                                    at: cursor,
                                    depth: child_depth,
                                });
                                cursor = cursor
                                    .wrapping_add(flat_extent(&program, child, cursor) as usize);
                            }
                            work.extend(children.into_iter().rev());
                        }
                        Node::FlatSequence {
                            offset,
                            stride,
                            element,
                        } => {
                            let count = if *offset == 16 {
                                word(at.add(8))
                            } else {
                                if *stride == 0 {
                                    fault("flat sequence without count");
                                }
                                word(at) / *stride
                            };
                            work.push(Job::Sequential {
                                node: *element,
                                at: at.add(*offset as usize),
                                depth: child_depth,
                                count,
                            });
                        }
                        Node::Sequential {
                            offset, element, ..
                        } => {
                            let at = payload(pointer(at));
                            let count = word(at);
                            work.push(Job::Sequential {
                                node: *element,
                                at: at.add(*offset as usize),
                                depth: child_depth,
                                count,
                            });
                        }
                        Node::Columns { element, .. } => {
                            let at = payload(pointer(at));
                            let (count, capacity) = (word(at), word(at.add(8)));
                            if count != 0 {
                                let Some(depth) =
                                    column_field_depth(&program, edges, *element, child_depth)
                                else {
                                    return false;
                                };
                                work.push(Job::Rows {
                                    node,
                                    at,
                                    depth,
                                    row: 0,
                                    count,
                                    capacity,
                                });
                            }
                        }
                        Node::Container(container) => {
                            let container_at = payload(pointer(at));
                            let at =
                                payload(pointer(container_at.add(container.entries_at as usize)));
                            let count = word(at);
                            work.push(Job::Entries {
                                container: *container,
                                at: at.add(container.entries.offset as usize),
                                depth: child_depth,
                                count,
                            });
                        }
                    }
                }
            }
        }
    }
    true
}
