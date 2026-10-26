//! In-place interpretation of the validated codec word program. Node tables,
//! fields and arms stay borrowed; child cursors retain one sibling continuation
//! rather than materializing a value's width in a worklist.
use super::*;

#[derive(Clone, Copy)]
pub(super) struct Program<'a> {
    words: &'a [u64],
}
impl<'a> Program<'a> {
    pub(super) fn new(words: &'a [u64]) -> Self {
        if words.len() < 7
            || words[0] != CODEC_VERSION
            || words[1] as usize > words.len() - 7
            || words[3] >= words[1]
        {
            fault("malformed borrowed access program");
        }
        Self { words }
    }
    pub(super) fn root(self) -> usize {
        self.words[3] as usize
    }
    pub(super) fn count(self) -> usize {
        self.words[1] as usize
    }
    pub(super) fn node(self, id: usize) -> &'a [u64] {
        if id >= self.count() {
            fault("access node outside program");
        }
        let start = self.words[7 + id] as usize;
        self.words
            .get(start..)
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| fault("access node offset"))
    }
}

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

/// The ordinal/count belong to the logical value. Box, checked and framing
/// nodes remain explicit for preflight, and value operations unwrap them.
#[derive(Clone, Copy)]
pub(super) struct Children<'a> {
    pub(super) ordinal: Option<u32>,
    pub(super) count: Option<u64>,
    pub(super) set: bool,
    kind: Cursor<'a>,
}
#[derive(Clone, Copy)]
enum Cursor<'a> {
    Empty,
    One(Projection),
    Fields {
        fields: &'a [u64],
        at: *const u8,
        flat: bool,
        previous: Option<usize>,
    },
    Elements {
        node: usize,
        at: *const u8,
        count: u64,
        stride: Option<u64>,
        previous: bool,
    },
    Columns {
        fields: &'a [u64],
        data: u64,
        base: *const u8,
        capacity: u64,
        row: u64,
    },
    Rows {
        node: usize,
        columns: usize,
        base: *const u8,
        count: u64,
        capacity: u64,
        row: u64,
    },
    Entries {
        node: &'a [u64],
        at: *const u8,
        count: u64,
        value: bool,
    },
}
impl<'a> Children<'a> {
    fn new(kind: Cursor<'a>) -> Self {
        Self {
            ordinal: None,
            count: None,
            set: false,
            kind,
        }
    }
    pub(super) fn has_more(self) -> bool {
        match self.kind {
            Cursor::Empty => false,
            Cursor::One(_) => true,
            Cursor::Fields { fields, .. } | Cursor::Columns { fields, .. } => !fields.is_empty(),
            Cursor::Elements { count, .. } | Cursor::Entries { count, .. } => count != 0,
            Cursor::Rows { row, count, .. } => row != count,
        }
    }
    /// # Safety
    /// The program and original operand validate all projected addresses.
    pub(super) unsafe fn next(&mut self, p: Program<'a>) -> Option<Projection> {
        unsafe {
            match &mut self.kind {
                Cursor::Empty => None,
                Cursor::One(child) => {
                    let result = *child;
                    self.kind = Cursor::Empty;
                    Some(result)
                }
                Cursor::Fields {
                    fields,
                    at,
                    flat,
                    previous,
                } => {
                    if fields.is_empty() {
                        return None;
                    }
                    if let Some(child) = previous.take() {
                        *at = at.wrapping_add(flat_extent(p, child, *at) as usize);
                    }
                    let (offset, child, n) = if *flat {
                        (0, fields[0] as usize, 1)
                    } else {
                        (fields[0], fields[1] as usize, 2)
                    };
                    let result = Projection::at(child, at.wrapping_add(offset as usize));
                    *fields = &fields[n..];
                    if *flat {
                        *previous = Some(child);
                    }
                    Some(result)
                }
                Cursor::Elements {
                    node,
                    at,
                    count,
                    stride,
                    previous,
                } => {
                    if *count == 0 {
                        return None;
                    }
                    if *previous {
                        *at = at.wrapping_add(
                            stride.unwrap_or_else(|| flat_extent(p, *node, *at)) as usize
                        );
                    }
                    let result = Projection::at(*node, *at);
                    *count -= 1;
                    *previous = true;
                    Some(result)
                }
                Cursor::Columns {
                    fields,
                    data,
                    base,
                    capacity,
                    row,
                } => {
                    if fields.is_empty() {
                        return None;
                    }
                    let result = Projection::at(
                        fields[2] as usize,
                        base.add((*data + *capacity * fields[0] + *row * fields[1]) as usize),
                    );
                    *fields = &fields[3..];
                    Some(result)
                }
                Cursor::Rows {
                    node,
                    columns,
                    base,
                    count,
                    capacity,
                    row,
                } => {
                    if *row == *count {
                        return None;
                    }
                    let result = Projection {
                        node: *node,
                        location: Location::Row {
                            columns: *columns,
                            base: *base,
                            capacity: *capacity,
                            row: *row,
                        },
                    };
                    *row += 1;
                    Some(result)
                }
                Cursor::Entries {
                    node,
                    at,
                    count,
                    value,
                } => {
                    while *count != 0 {
                        let storage = MapStorage {
                            hashes_at: node[17],
                            live_at: node[18],
                            tag_at: node[19],
                            tag_form: node[20],
                            tag_bit: node[21],
                        };
                        if !storage.live(*at) {
                            *at = at.wrapping_add(node[9] as usize);
                            *count -= 1;
                            continue;
                        }
                        let result = if *value {
                            Projection::at(node[15] as usize, at.add(node[16] as usize))
                        } else {
                            Projection::at(node[13] as usize, at.add(node[14] as usize))
                        };
                        if !*value && node[15] != NO_NODE {
                            *value = true;
                        } else {
                            *value = false;
                            *at = at.wrapping_add(node[9] as usize);
                            *count -= 1;
                        }
                        return Some(result);
                    }
                    None
                }
            }
        }
    }
}

fn arm(mut words: &[u64], ordinal: u32, width: usize) -> &[u64] {
    for _ in 0..ordinal {
        words = &words[1 + words[0] as usize * width..];
    }
    &words[1..1 + words[0] as usize * width]
}

/// One explicit projection step. Wrappers have one child and need no sibling
/// frame, including arbitrarily many transparent representation wrappers.
pub(super) unsafe fn children<'a>(p: Program<'a>, projection: Projection) -> Children<'a> {
    unsafe {
        let node = p.node(projection.node);
        if node[0] == NODE_CHECKED {
            return Children::new(Cursor::One(Projection {
                node: node[2] as usize,
                ..projection
            }));
        }
        if let Location::Row {
            columns,
            base,
            capacity,
            row,
        } = projection.location
        {
            if node[0] != NODE_FIELDS {
                fault("access row without fields");
            }
            let columns = p.node(columns);
            return Children::new(Cursor::Columns {
                fields: &columns[7..7 + columns[6] as usize * 3],
                data: columns[3],
                base,
                capacity,
                row,
            });
        }
        let Location::Address(at) = projection.location else {
            unreachable!()
        };
        let mut result = Children::new(match node[0] {
            NODE_SCALAR | NODE_TEXT | NODE_FLAT_TEXT | NODE_PATH => Cursor::Empty,
            NODE_FRAMED => Cursor::One(Projection::at(node[1] as usize, at.add(8))),
            NODE_BOX => Cursor::One(Projection::at(node[4] as usize, payload(pointer(at)))),
            NODE_FIELDS => Cursor::Fields {
                fields: &node[2..2 + node[1] as usize * 2],
                at,
                flat: false,
                previous: None,
            },
            NODE_FLAT_FIELDS => Cursor::Fields {
                fields: &node[2..2 + node[1] as usize],
                at,
                flat: true,
                previous: None,
            },
            NODE_VARIANT => {
                let ordinal = if node[1] != 0 {
                    if word(at.add(node[2] as usize)) == 0 {
                        node[3] as u32
                    } else {
                        1 - node[3] as u32
                    }
                } else {
                    at.add(node[2] as usize).cast::<u32>().read_unaligned()
                };
                if u64::from(ordinal) >= node[4] {
                    fault("access variant tag");
                }
                let mut c = Children::new(Cursor::Fields {
                    fields: arm(&node[5..], ordinal, 2),
                    at,
                    flat: false,
                    previous: None,
                });
                c.ordinal = Some(ordinal);
                return c;
            }
            NODE_FLAT_VARIANT => {
                let ordinal = at.cast::<u32>().read_unaligned();
                if u64::from(ordinal) >= node[1] {
                    fault("access flat variant tag");
                }
                let mut c = Children::new(Cursor::Fields {
                    fields: arm(&node[2..], ordinal, 1),
                    at: at.add(4),
                    flat: true,
                    previous: None,
                });
                c.ordinal = Some(ordinal);
                return c;
            }
            NODE_REPEAT => Cursor::Elements {
                node: node[3] as usize,
                at,
                count: node[1],
                stride: Some(node[2]),
                previous: false,
            },
            NODE_SEQUENCE => {
                let base = payload(pointer(at));
                Cursor::Elements {
                    node: node[5] as usize,
                    at: base.add(node[3] as usize),
                    count: word(base),
                    stride: Some(node[4]),
                    previous: false,
                }
            }
            NODE_FLAT_SEQUENCE => {
                let count = if node[1] == 16 {
                    word(at.add(8))
                } else {
                    word(at) / node[2]
                };
                Cursor::Elements {
                    node: node[3] as usize,
                    at: at.add(node[1] as usize),
                    count,
                    stride: None,
                    previous: false,
                }
            }
            NODE_SEQUENTIAL => {
                let base = payload(pointer(at));
                Cursor::Elements {
                    node: node[4] as usize,
                    at: base.add(node[3] as usize),
                    count: word(base),
                    stride: None,
                    previous: false,
                }
            }
            NODE_COLUMNS => {
                let base = payload(pointer(at));
                Cursor::Rows {
                    node: node[5] as usize,
                    columns: projection.node,
                    base,
                    count: word(base),
                    capacity: word(base.add(8)),
                    row: 0,
                }
            }
            NODE_MAP | NODE_SET => {
                let base = payload(pointer(payload(pointer(at)).add(node[4] as usize)));
                Cursor::Entries {
                    node,
                    at: base.add(node[8] as usize),
                    count: word(base),
                    value: false,
                }
            }
            _ => fault("unknown borrowed access node"),
        });
        result.count = match result.kind {
            Cursor::Elements { count, .. } | Cursor::Rows { count, .. } => Some(count),
            Cursor::Entries { .. } => {
                // Logical length counts only live entries, including after removals.
                let mut entries = result;
                let mut count = 0;
                while entries.next(p).is_some() {
                    count += 1;
                }
                Some(if node[0] == NODE_MAP {
                    count / 2
                } else {
                    count
                })
            }
            _ => None,
        };
        result.set = node[0] == NODE_SET;
        result
    }
}

/// Flat extents use the emitted framing/active-arm sizes. Unframed field
/// concatenations are traversed with one sibling slice per containing record.
unsafe fn flat_extent(p: Program<'_>, root: usize, at: *const u8) -> u64 {
    let mut work = [(&[][..], 0usize); hirsel_core_contract::depth::STRUCTURAL_LIMIT as usize + 1];
    let mut used = 0;
    let mut node = root;
    let mut extent = 0u64;
    loop {
        let words = p.node(node);
        let cursor = at.wrapping_add(extent as usize);
        // SAFETY: the caller established an initialized flat value of this node.
        unsafe {
            let n = match words[0] {
                NODE_SCALAR => match words[1] {
                    0 | 1 | 6 => 1,
                    2 | 7 => 2,
                    3 | 8 | 11 | 13 => 4,
                    4 | 9 | 12 => 8,
                    5 | 10 => 16,
                    _ => fault("access scalar"),
                },
                NODE_FLAT_TEXT | NODE_FRAMED | NODE_FLAT_SEQUENCE => word(cursor)
                    .checked_add(8)
                    .unwrap_or_else(|| fault("flat extent overflow")),
                NODE_CHECKED => {
                    node = words[2] as usize;
                    continue;
                }
                NODE_VARIANT => {
                    let ordinal = cursor.add(words[2] as usize).cast::<u32>().read_unaligned();
                    if u64::from(ordinal) >= words[4] {
                        fault("Packed extent tag");
                    }
                    let mut rest = &words[5..];
                    for _ in 0..words[4] {
                        rest = &rest[1 + rest[0] as usize * 2..];
                    }
                    rest[ordinal as usize]
                }
                NODE_REPEAT => words[1]
                    .checked_mul(words[2])
                    .unwrap_or_else(|| fault("flat extent overflow")),
                NODE_FIELDS | NODE_FLAT_FIELDS | NODE_FLAT_VARIANT => {
                    let (fields, width, prefix) = match words[0] {
                        NODE_FIELDS => (&words[2..2 + words[1] as usize * 2], 2, 0),
                        NODE_FLAT_FIELDS => (&words[2..2 + words[1] as usize], 1, 0),
                        _ => {
                            let ordinal = cursor.cast::<u32>().read_unaligned();
                            if u64::from(ordinal) >= words[1] {
                                fault("flat extent tag");
                            }
                            (arm(&words[2..], ordinal, 1), 1, 4)
                        }
                    };
                    extent += prefix;
                    if !fields.is_empty() {
                        node = fields[width - 1] as usize;
                        if fields.len() > width {
                            if used == work.len() {
                                fault("flat extent exceeds admitted depth");
                            }
                            work[used] = (&fields[width..], width);
                            used += 1;
                        }
                        continue;
                    }
                    0
                }
                _ => fault("non-flat access extent"),
            };
            extent = extent
                .checked_add(n)
                .unwrap_or_else(|| fault("flat extent overflow"));
        }
        if used == 0 {
            return extent;
        }
        let (fields, width) = &mut work[used - 1];
        node = fields[*width - 1] as usize;
        *fields = &fields[*width..];
        if fields.is_empty() {
            used -= 1;
        }
    }
}

#[cfg(test)]
mod tests;
