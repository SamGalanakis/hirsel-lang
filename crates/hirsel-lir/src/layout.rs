//! Physical layout arithmetic and scan/drop program operations.
use crate::layout_types::*;
use crate::runtime_abi::HEADER_SIZE;
use crate::snapshot;
use hirsel_syntax::ContentHash;

pub const FLAT_PREFIX: u64 = 8;
pub const MAX_DEPTH: u32 = 256;

/// Why a layout could not be derived.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LayoutError {
    /// Native-unavailable: an open spec question (`issue`) or a feature
    /// outside M1b. A tool outcome, never a source verdict.
    Unavailable { issue: Option<u32>, what: String },
    /// A generic parameter without an actual; instances must be complete.
    Incomplete,
    /// A TypeId outside its store.
    MissingType,
    /// No owner representation for a nominal or container.
    MissingOwner(String),
    /// Malformed input the front end should have rejected.
    Invalid(String),
    /// Checked size or offset arithmetic overflowed.
    Overflow,
    /// An inline recursive path that never passes through an explicit block.
    InfiniteSize(String),
    /// The tooling limit on instance expansion.
    InstanceLimit,
    /// A dynamic size program read outside its value or met a malformed term.
    Extent(String),
}

fn invalid<T>(what: impl Into<String>) -> Result<T, LayoutError> {
    Err(LayoutError::Invalid(what.into()))
}

/// Checked `align_up`; `align` must be a power of two.
pub fn align_up(offset: u64, align: u64) -> Result<u64, LayoutError> {
    if !align.is_power_of_two() {
        return invalid(format!("alignment {align} is not a power of two"));
    }
    offset
        .checked_add(align - 1)
        .map(|n| n & !(align - 1))
        .ok_or(LayoutError::Overflow)
}

/// The offset of a `Sequence` payload's capacity word (runtime ABI 5): the u64 number of
/// elements its storage holds, after the u64 count at offset 0. It is never more than the
/// storage's real slots, and exact after every `SeqReserve`, so a capacity test is one load.
pub const SEQUENCE_CAPACITY_OFFSET: u64 = 8;

/// The offset of a `Sequence` payload's first element of layout `element`: past
/// the u64 count at offset 0 and the u64 capacity at offset 8, aligned to the
/// element (runtime ABI 5). The layout builder and validator, and any harness
/// reading elements, take it from here.
pub fn sequence_elements_offset(element: &LayoutRecord) -> Result<u64, LayoutError> {
    align_up(16, u64::from(element.align))
}

/// The offset from a slice owner's payload that the slice's start word counts from: past
/// one u64 word, aligned to the element. A flat view's start is its encoding's offset,
/// whose u64 length prefix that word skips.
pub fn slice_data_offset(element: &LayoutRecord) -> Result<u64, LayoutError> {
    align_up(8, u64::from(element.align))
}

/// The start word of a slice over a whole `Sequence` of `element`: its first element's
/// offset past [`slice_data_offset`].
pub fn sequence_slice_start(element: &LayoutRecord) -> Result<u64, LayoutError> {
    Ok(sequence_elements_offset(element)? - slice_data_offset(element)?)
}

/// The offset of a `Sequential` payload's first element of layout `element` (MEM-13):
/// past the u64 count at offset 0 and the u64 used-bytes word at offset 8, aligned to the
/// element. The layout builder and validator, emission and the heap checker take it from
/// here.
pub fn sequential_elements_offset(element: &LayoutRecord) -> Result<u64, LayoutError> {
    align_up(16, u64::from(element.align))
}

/// The offset of an `@soa` sequence payload's first column, whose fields' largest
/// alignment is `quantum`: past the u64 length and u64 capacity words, aligned to it. The
/// layout builder and validator take it from here; every other reader takes the record's
/// `SoaColumns::data_offset`.
pub fn soa_data_offset(quantum: u64) -> Result<u64, LayoutError> {
    align_up(16, quantum)
}

/// Offset of a block payload of alignment `align` from its header: the
/// payload follows the counted header v2. No consumer hardcodes it.
pub fn payload_offset(align: u32) -> Result<u64, LayoutError> {
    if align == 0 || !align.is_power_of_two() {
        return Err(LayoutError::Invalid(
            "payload alignment is not a power of two".into(),
        ));
    }
    Ok(HEADER_SIZE)
}

/// The least extent a size program gives: every length it reads zero. A flat
/// builder's reservation of `n` elements requests `n` of its element's least
/// extent, the prefix plus this of the content (LIR schema revision 22).
pub fn least_size(size: &SizeSpec) -> u64 {
    evaluate_size(size, &[0; 4096]).unwrap_or(0)
}

/// Evaluate a size against the value's bytes, every step checked.
pub fn evaluate_size(size: &SizeSpec, bytes: &[u8]) -> Result<u64, LayoutError> {
    match size {
        SizeSpec::Fixed(n) => Ok(*n),
        SizeSpec::Dynamic(program) => run(program, bytes, 0, 0),
    }
}

/// The byte extent of the value at the start of `bytes`.
/// The constant extent of each variant of a dynamic `Packed[T]` record, whose
/// u32 ordinal tag lies at offset 0: the steps a sequential sequence walks by.
/// None for any other record.
pub fn packed_extents(record: &LayoutRecord) -> Option<Vec<(u32, u64)>> {
    let tagged = record
        .tag
        .as_ref()
        .is_some_and(|t| t.offset == 0 && t.form == TagForm::Ordinal);
    let (LayoutKind::Packed, SizeSpec::Dynamic(program), true) =
        (&record.kind, &record.size, tagged)
    else {
        return None;
    };
    let [
        SizeTerm::ByTag {
            tag_offset: 0,
            variants,
        },
    ] = program.as_slice()
    else {
        return None;
    };
    variants
        .iter()
        .map(|(ordinal, p)| match p.as_slice() {
            [SizeTerm::Constant(n)] => Some((*ordinal, *n)),
            _ => None,
        })
        .collect()
}

pub fn extent(table: &LayoutTable, id: LayoutId, bytes: &[u8]) -> Result<u64, LayoutError> {
    let record = table
        .layout(id)
        .ok_or_else(|| LayoutError::Extent(format!("no layout {}", id.0)))?;
    evaluate_size(&record.size, bytes)
}

fn run(program: &[SizeTerm], bytes: &[u8], base: u64, nesting: u32) -> Result<u64, LayoutError> {
    let bad = |what: &str| LayoutError::Extent(what.into());
    if nesting > MAX_DEPTH {
        return Err(bad("size program nests too deeply"));
    }
    let mut stack: Vec<u64> = Vec::new();
    let pop = |stack: &mut Vec<u64>| stack.pop().ok_or_else(|| bad("size stack underflow"));
    for term in program {
        match term {
            SizeTerm::Constant(n) => stack.push(*n),
            SizeTerm::ReadLength { offset, scalar } => {
                let at = base.checked_add(*offset).ok_or(LayoutError::Overflow)?;
                stack.push(read_unsigned(bytes, at, *scalar)?);
            }
            SizeTerm::Add => {
                let (b, a) = (pop(&mut stack)?, pop(&mut stack)?);
                stack.push(a.checked_add(b).ok_or(LayoutError::Overflow)?);
            }
            SizeTerm::Multiply => {
                let (b, a) = (pop(&mut stack)?, pop(&mut stack)?);
                stack.push(a.checked_mul(b).ok_or(LayoutError::Overflow)?);
            }
            SizeTerm::AlignUp(align) => {
                let a = pop(&mut stack)?;
                stack.push(align_up(a, *align)?);
            }
            SizeTerm::Dup => {
                let a = *stack.last().ok_or_else(|| bad("size stack underflow"))?;
                stack.push(a);
            }
            SizeTerm::At(inner) => {
                let offset = pop(&mut stack)?;
                let at = base.checked_add(offset).ok_or(LayoutError::Overflow)?;
                stack.push(run(inner, bytes, at, nesting + 1)?);
            }
            SizeTerm::ByTag {
                tag_offset,
                variants,
            } => {
                let at = base.checked_add(*tag_offset).ok_or(LayoutError::Overflow)?;
                let tag = read_unsigned(bytes, at, ScalarKind::U32)? as u32;
                let (_, inner) = variants
                    .iter()
                    .find(|(ordinal, _)| *ordinal == tag)
                    .ok_or_else(|| bad("size program has no entry for the active tag"))?;
                stack.push(run(inner, bytes, base, nesting + 1)?);
            }
        }
    }
    match stack.as_slice() {
        [n] => Ok(*n),
        _ => Err(bad("size program does not leave exactly one value")),
    }
}

fn read_unsigned(bytes: &[u8], at: u64, scalar: ScalarKind) -> Result<u64, LayoutError> {
    let width = match scalar {
        ScalarKind::U8 => 1,
        ScalarKind::U16 => 2,
        ScalarKind::U32 => 4,
        ScalarKind::U64 => 8,
        _ => return Err(LayoutError::Extent("length scalar is not unsigned".into())),
    };
    let start = usize::try_from(at).map_err(|_| LayoutError::Overflow)?;
    let end = start.checked_add(width).ok_or(LayoutError::Overflow)?;
    let slice = bytes
        .get(start..end)
        .ok_or_else(|| LayoutError::Extent(format!("length read at {at} is out of bounds")))?;
    let mut buf = [0u8; 8];
    buf[..width].copy_from_slice(slice);
    Ok(u64::from_le_bytes(buf))
}

pub fn scan_order(step: &ScanStep) -> u64 {
    match step {
        ScanStep::Counted { offset }
        | ScanStep::Uncounted { offset }
        | ScanStep::SliceOwner { offset }
        | ScanStep::Inline { offset, .. }
        | ScanStep::Sequence { offset, .. }
        | ScanStep::Environment { offset }
        | ScanStep::Handle { offset }
        | ScanStep::Repeat { offset, .. } => *offset,
        ScanStep::ActiveVariant { tag_offset, .. } => *tag_offset,
        ScanStep::Columns(c) => c.count_offset,
    }
}

fn shift_columns(c: &SoaColumns, by: u64) -> SoaColumns {
    SoaColumns {
        count_offset: c.count_offset + by,
        capacity_offset: c.capacity_offset + by,
        data_offset: c.data_offset + by,
        ..c.clone()
    }
}

/// The scan program of a borrowed view of `child` held inline at `by` (a
/// closure environment's borrowed capture): every counted, slice-owner or
/// environment reference scanned as uncounted, handles as handles, inline
/// children and repeats flattened, variants by their active steps. Such a
/// field is never dropped nor copied with its holder. None when `child` holds
/// a sequence or columns inline, which no view does.
pub fn borrowed_scan<'r>(
    child: &LayoutRecord,
    by: u64,
    record: &impl Fn(LayoutId) -> &'r LayoutRecord,
) -> Option<Vec<ScanStep>> {
    let mut out = Vec::new();
    for step in &child.scan {
        match step {
            ScanStep::Counted { offset }
            | ScanStep::Uncounted { offset }
            | ScanStep::SliceOwner { offset }
            | ScanStep::Environment { offset } => out.push(ScanStep::Uncounted {
                offset: offset + by,
            }),
            ScanStep::Handle { offset } => out.push(ScanStep::Handle {
                offset: offset + by,
            }),
            ScanStep::Inline { offset, layout } => {
                out.extend(borrowed_scan(record(*layout), by + offset, record)?)
            }
            ScanStep::Repeat {
                offset,
                count,
                stride,
                element,
            } => {
                for i in 0..*count {
                    out.extend(borrowed_scan(
                        record(*element),
                        by + offset + i * stride,
                        record,
                    )?);
                }
            }
            ScanStep::ActiveVariant {
                tag_offset,
                form,
                variants,
            } => {
                let mut mapped = Vec::new();
                for (ordinal, steps) in variants {
                    let inner = LayoutRecord {
                        scan: steps.clone(),
                        ..child.clone()
                    };
                    mapped.push((*ordinal, borrowed_scan(&inner, by, record)?));
                }
                out.push(ScanStep::ActiveVariant {
                    tag_offset: tag_offset + by,
                    form: *form,
                    variants: mapped,
                });
            }
            ScanStep::Sequence { .. } | ScanStep::Columns(_) => return None,
        }
    }
    Some(out)
}

/// Whether this field holds a view rather than owning its child's holders.
/// Scan and drop must agree. Construction and closure captures use this same
/// evidence; a missing drop alone never turns an owned field into a borrow.
pub fn field_is_borrowed(table: &LayoutTable, parent: &LayoutRecord, field: &FieldLayout) -> bool {
    if matches!(&parent.kind, LayoutKind::Environment { movable } if movable.contains(&field.ordinal))
    {
        return false;
    }
    // Variant payloads are owned, with their recipes nested in ActiveVariant.
    // Only a declared inline field can carry this scan/drop evidence for a view.
    if !parent.fields.contains(field) {
        return false;
    }
    let Some(child) = table.layout(field.layout) else {
        return false;
    };
    let Some(borrowed) = borrowed_scan(child, field.offset, &|id| &table.layouts[id.0 as usize])
    else {
        return false;
    };
    let owned: Vec<_> = child
        .scan
        .iter()
        .map(|x| shift_scan(x, field.offset))
        .collect();
    let dropped: Vec<_> = if child.finalizer.is_some() {
        vec![DropStep::Child {
            offset: field.offset,
            layout: field.layout,
        }]
    } else {
        child
            .drop
            .iter()
            .map(|x| shift_drop(x, field.offset))
            .collect()
    };
    let by_scan = borrowed != owned
        && borrowed.iter().all(|x| parent.scan.contains(x))
        && !owned
            .iter()
            .any(|x| !borrowed.contains(x) && parent.scan.contains(x));
    let by_drop = !dropped.is_empty() && !dropped.iter().any(|x| parent.drop.contains(x));
    (by_scan || borrowed == owned) && (by_drop || dropped.is_empty()) && (by_scan || by_drop)
}

/// Re-base a scan step into a parent at `by`. Offsets were checked when the
/// parent's fields were placed, so the sums cannot overflow.
pub fn shift_scan(step: &ScanStep, by: u64) -> ScanStep {
    match step {
        ScanStep::Counted { offset } => ScanStep::Counted {
            offset: offset + by,
        },
        ScanStep::Uncounted { offset } => ScanStep::Uncounted {
            offset: offset + by,
        },
        ScanStep::SliceOwner { offset } => ScanStep::SliceOwner {
            offset: offset + by,
        },
        ScanStep::Inline { offset, layout } => ScanStep::Inline {
            offset: offset + by,
            layout: *layout,
        },
        ScanStep::ActiveVariant {
            tag_offset,
            form,
            variants,
        } => ScanStep::ActiveVariant {
            tag_offset: tag_offset + by,
            form: *form,
            variants: variants
                .iter()
                .map(|(o, steps)| (*o, steps.iter().map(|s| shift_scan(s, by)).collect()))
                .collect(),
        },
        ScanStep::Sequence {
            count_offset,
            offset,
            stride,
            element,
        } => ScanStep::Sequence {
            count_offset: count_offset + by,
            offset: offset + by,
            stride: *stride,
            element: *element,
        },
        ScanStep::Environment { offset } => ScanStep::Environment {
            offset: offset + by,
        },
        ScanStep::Handle { offset } => ScanStep::Handle {
            offset: offset + by,
        },
        ScanStep::Repeat {
            offset,
            count,
            stride,
            element,
        } => ScanStep::Repeat {
            offset: offset + by,
            count: *count,
            stride: *stride,
            element: *element,
        },
        ScanStep::Columns(c) => ScanStep::Columns(shift_columns(c, by)),
    }
}

pub fn shift_drop(step: &DropStep, by: u64) -> DropStep {
    match step {
        DropStep::Finalize => DropStep::Finalize,
        DropStep::Release { offset } => DropStep::Release {
            offset: offset + by,
        },
        DropStep::Child { offset, layout } => DropStep::Child {
            offset: offset + by,
            layout: *layout,
        },
        DropStep::ActiveVariant {
            tag_offset,
            form,
            variants,
        } => DropStep::ActiveVariant {
            tag_offset: tag_offset + by,
            form: *form,
            variants: variants
                .iter()
                .map(|(o, steps)| (*o, steps.iter().map(|s| shift_drop(s, by)).collect()))
                .collect(),
        },
        DropStep::ElementsReverse {
            count_offset,
            offset,
            stride,
            element,
        } => DropStep::ElementsReverse {
            count_offset: count_offset + by,
            offset: offset + by,
            stride: *stride,
            element: *element,
        },
        DropStep::Environment { offset } => DropStep::Environment {
            offset: offset + by,
        },
        DropStep::CloseHandle { offset } => DropStep::CloseHandle {
            offset: offset + by,
        },
        DropStep::RepeatReverse {
            offset,
            count,
            stride,
            element,
        } => DropStep::RepeatReverse {
            offset: offset + by,
            count: *count,
            stride: *stride,
            element: *element,
        },
        DropStep::ColumnsReverse(c) => DropStep::ColumnsReverse(shift_columns(c, by)),
    }
}

impl LayoutTable {
    /// The canonical snapshot digest of one record.
    pub fn record_digest(&self, id: LayoutId) -> Option<ContentHash> {
        self.layout(id)
            .map(|r| snapshot::digest("layout-record", r))
    }
}
