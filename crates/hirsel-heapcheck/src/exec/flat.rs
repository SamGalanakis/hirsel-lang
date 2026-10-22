//! FlatFormatV0 values (LIR schema revision 4): dynamic extents read from
//! their size programs, sequence elements at their cursors, the views and
//! counts of variable parts, and flat constructions that encode String,
//! Bytes, sequences and variable flat record values behind their u64 length prefixes.
use super::*;
use hirsel_lir::layout_types::SizeTerm;

/// One live flat payload in H's checked backing ledger. The block handle and
/// runtime header are outside these bytes; spare allocation capacity is reported
/// separately. Cells are typed handles, never inferred from address-like bytes.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FlatStorage {
    pub alloc: AllocId,
    pub generation: u32,
    pub owner: Owner,
    pub layout: LayoutId,
    pub capacity: u64,
    pub bytes: Vec<u8>,
    pub initialized: bool,
    pub cell_offsets: Vec<u64>,
    pub resources: usize,
}

/// Opt-in physical instrumentation for bounded layout tests. Observe live flat
/// backing payloads after each executor step, including partially initialized
/// constructions. This does not change execution, allocation or trace formats.
/// The callback receives the step, flat payloads, and IDs of *all* live
/// heap/region blocks, allowing a test to detect separately retained parts.
/// Frame/temporary/static storage is excluded from the last list. Snapshots
/// are transient and allocated only by this explicitly selected test path.
/// Normal and injected exits receive the same terminal audit as `run`.
pub fn run_with_flat_storage_observer<S: Stage>(
    program: &Program<S>,
    entry: &hirsel_syntax::DefPath,
    args: &[Arg],
    ops: &mut dyn Operations,
    config: &Config,
    observe: &mut dyn FnMut(u64, &[FlatStorage], &[AllocId]),
) -> crate::Run {
    let mut machine = Machine::new(program, config, ops);
    let result = (|| {
        program.check()?;
        machine.start(program.entry(entry)?, args)?;
        while machine.step()? {
            let storage: Vec<_> = machine
                .ledger
                .allocations
                .indexed()
                .filter_map(|(id, a)| {
                    let layout = a.layout?;
                    if a.state != AllocState::Live
                        || program.layouts.layout(layout)?.kind != LayoutKind::Flat
                    {
                        return None;
                    }
                    Some(FlatStorage {
                        alloc: AllocId(id as u32),
                        generation: a.generation,
                        owner: a.owner,
                        layout,
                        capacity: a.capacity,
                        bytes: a.extent.read_bytes(0, a.size).to_vec(),
                        initialized: a.extent.all_init(0, a.size),
                        cell_offsets: a.extent.cells.keys().copied().collect(),
                        resources: a.extent.resources.len(),
                    })
                })
                .collect();
            // All live heap/region blocks, including non-flat construction
            // sources, so tests can distinguish transients from retained parts.
            let blocks: Vec<_> = machine
                .ledger
                .allocations
                .indexed()
                .filter(|(_, a)| {
                    a.state == AllocState::Live && matches!(a.owner, Owner::Heap | Owner::Region(_))
                })
                .map(|(id, _)| AllocId(id as u32))
                .collect();
            observe(machine.steps, &storage, &blocks);
        }
        let (observation, exemptions) = machine.finish()?;
        Ok(crate::Audit {
            observation,
            exemptions,
        })
    })()
    .map_err(|v: Box<crate::Violation>| *v);
    crate::Run {
        result: crate::blocked(result, &machine.output, machine.signaled),
        output: machine.output.clone(),
        sites: machine.sites,
        choices: machine.choices,
        steps: machine.steps,
        trace: machine.ledger.trace.clone(),
        counters: machine.ledger.counters,
    }
}

/// The u64 length prefix of a variable FlatFormatV0 part.
const PREFIX: u64 = 8;

impl<S: Stage> Machine<'_, S> {
    /// A length-prefixed encoding: a dynamic Flat record whose one field, its
    /// content, follows the prefix (offset 8) or the prefix and the element
    /// count of a list of variable elements (offset 16).
    pub(super) fn flat_prefixed(&self, layout: LayoutId) -> Checked<Option<(u64, LayoutId)>> {
        let r = self.record(layout)?;
        Ok(match (&r.kind, &r.size, r.fields.as_slice()) {
            (LayoutKind::Flat, SizeSpec::Dynamic(_), [only]) if matches!(only.offset, 8 | 16) => {
                Some((only.offset, only.layout))
            }
            _ => None,
        })
    }

    /// Where the field `field` of the flat record `record` lies when an open
    /// element builds it in place (LIR schema revision 24): a byte sequence
    /// after only fixed fields, at its static offset.
    pub(super) fn open_field(&self, record: LayoutId, field: u32) -> Checked<u64> {
        let r = self.record(record)?;
        let Some(f) = r.fields.iter().find(|f| f.ordinal == field) else {
            return fail(V::TypeConfusion, "an open element of a missing field");
        };
        for e in r.fields.iter().filter(|e| e.ordinal < field) {
            if !matches!(self.record(e.layout)?.size, SizeSpec::Fixed(_)) {
                return fail(V::TypeConfusion, "an open field after a variable one");
            }
        }
        let bytes = match self.flat_sequence(f.layout)? {
            Some(element) => self.record(element)?.kind == LayoutKind::Scalar(ScalarKind::U8),
            None => false,
        };
        if r.kind != LayoutKind::Flat || !bytes {
            return fail(
                V::TypeConfusion,
                "an open field that is not a byte sequence",
            );
        }
        Ok(f.offset)
    }

    /// The element of a flat sequence encoding (List, Array, String, Bytes).
    pub(super) fn flat_sequence(&self, layout: LayoutId) -> Checked<Option<LayoutId>> {
        Ok(match self.flat_prefixed(layout)? {
            Some((16, element)) => Some(element),
            Some((_, element)) if matches!(self.record(element)?.size, SizeSpec::Fixed(_)) => {
                Some(element)
            }
            _ => None,
        })
    }

    /// The byte extent of a value of `layout` stored at `at`: its fixed size,
    /// or its size program evaluated over the stored bytes.
    pub(super) fn extent_of(&self, layout: LayoutId, at: Ptr) -> Checked<u64> {
        match &self.record(layout)?.size {
            SizeSpec::Fixed(n) => Ok(*n),
            SizeSpec::Dynamic(program) => self.size_program(program, at),
        }
    }

    fn size_program(&self, program: &[SizeTerm], at: Ptr) -> Checked<u64> {
        let overflow = || err(V::Malformed, "a flat size program overflows");
        let mut stack: Vec<u64> = Vec::new();
        let pop = |stack: &mut Vec<u64>| {
            stack
                .pop()
                .ok_or_else(|| err(V::Malformed, "a flat size program underflows"))
        };
        for term in program {
            let value = match term {
                SizeTerm::Constant(n) => *n,
                SizeTerm::ReadLength { offset, scalar } => {
                    let size = u64::from(scalar_width(*scalar)?);
                    u64::try_from(self.read_scalar(at.at(*offset), size)?)
                        .map_err(|_| overflow())?
                }
                SizeTerm::Add => {
                    let (b, a) = (pop(&mut stack)?, pop(&mut stack)?);
                    a.checked_add(b).ok_or_else(overflow)?
                }
                SizeTerm::Multiply => {
                    let (b, a) = (pop(&mut stack)?, pop(&mut stack)?);
                    a.checked_mul(b).ok_or_else(overflow)?
                }
                SizeTerm::AlignUp(n) => {
                    let a = pop(&mut stack)?;
                    hirsel_lir::layout::align_up(a, *n).map_err(|_| overflow())?
                }
                SizeTerm::Dup => {
                    let a = *stack
                        .last()
                        .ok_or_else(|| err(V::Malformed, "a flat size program underflows"))?;
                    a
                }
                SizeTerm::At(inner) => {
                    let offset = pop(&mut stack)?;
                    self.size_program(inner, at.at(offset))?
                }
                SizeTerm::ByTag {
                    tag_offset,
                    variants,
                } => {
                    let tag = self.read_scalar(at.at(*tag_offset), 4)?;
                    let (_, inner) = variants
                        .iter()
                        .find(|(v, _)| u128::from(*v) == tag)
                        .ok_or_else(|| err(V::Malformed, format!("no size for tag {tag}")))?;
                    self.size_program(inner, at)?
                }
            };
            stack.push(value);
        }
        match stack.as_slice() {
            [n] => Ok(*n),
            _ => fail(V::Malformed, "a flat size program leaves no single size"),
        }
    }

    fn prefix_at(&self, at: Ptr) -> Checked<u64> {
        u64::try_from(self.read_scalar(at, PREFIX)?)
            .map_err(|_| err(V::Malformed, "a flat length prefix beyond u64"))
    }

    /// The element count of the flat sequence encoding of `layout` at `at`.
    pub(super) fn flat_count(&self, layout: LayoutId, at: Ptr) -> Checked<u64> {
        match self.flat_prefixed(layout)? {
            Some((16, _)) => self.prefix_at(at.at(PREFIX)),
            Some((_, element)) => {
                let bytes = self.prefix_at(at)?;
                match self.size(element)? {
                    0 => fail(V::Unsupported, "a flat sequence of empty elements"),
                    size if bytes % size != 0 => fail(
                        V::Malformed,
                        "a flat sequence's bytes are no whole elements",
                    ),
                    size => Ok(bytes / size),
                }
            }
            None => fail(
                V::TypeConfusion,
                "the length of a flat part that is no sequence",
            ),
        }
    }

    /// Element `index` of the flat sequence of `layout` at `at`, or None past
    /// its end: fixed elements by stride, variable ones walked past each
    /// earlier element's prefix.
    pub(super) fn flat_element(
        &mut self,
        layout: LayoutId,
        at: Ptr,
        index: u128,
    ) -> Checked<Option<(Ptr, LayoutId)>> {
        let Some(element) = self.flat_sequence(layout)? else {
            return fail(
                V::TypeConfusion,
                "an index into a flat part that is no sequence",
            );
        };
        if index >= u128::from(self.flat_count(layout, at)?) {
            return Ok(None);
        }
        let index = index as u64;
        Ok(Some(match self.flat_prefixed(layout)? {
            Some((16, _)) if !matches!(self.record(element)?.size, SizeSpec::Fixed(_)) => {
                let mut cursor = at.at(2 * PREFIX);
                for _ in 0..index {
                    self.ledger.counters.flat_prefix_reads += 1;
                    let length = self.prefix_at(cursor)?;
                    cursor = cursor.at(PREFIX
                        .checked_add(length)
                        .ok_or_else(|| err(V::Malformed, "a flat element extent overflows"))?);
                }
                (cursor, element)
            }
            Some((offset, _)) => (at.at(offset + index * self.size(element)?), element),
            None => return fail(V::Malformed, "a flat sequence without content"),
        }))
    }

    /// Check an IR offset against H's byte-derived boundary, even when cached.
    pub(super) fn flat_at(
        &mut self,
        layout: LayoutId,
        at: Ptr,
        index: u128,
        offset: u128,
    ) -> Checked<Option<(Ptr, LayoutId, u64)>> {
        let result = self.flat_boundary(layout, at, index)?;
        if let Some((element, _, _)) = result {
            let actual = element.offset - at.offset - 16;
            if u128::from(actual) != offset {
                return fail(
                    V::Malformed,
                    format!(
                        "flat element {index} lies {actual} bytes into its sequence, not {offset}"
                    ),
                );
            }
        }
        Ok(result)
    }

    /// Element `index` of the sequential Packed sequence of `element` at `at`
    /// (MEM-13), checked against the IR's `offset` from the first element: its
    /// place and its end from the first, or None past the count. Boundaries are
    /// H's own, walked from checked tags and cached as a flat walk's are.
    pub(super) fn sequential_at(
        &mut self,
        element: LayoutId,
        at: Ptr,
        index: u128,
        offset: u128,
    ) -> Checked<Option<(Ptr, u64)>> {
        let count = u64::try_from(self.read_scalar(at, PREFIX)?)
            .map_err(|_| err(V::Malformed, "a sequence count beyond u64"))?;
        if index >= u128::from(count) {
            return Ok(None);
        }
        let first = at.at(self.sequential_start(element)?);
        let index = index as usize;
        loop {
            let (len, last) = {
                let allocation = self.ledger.check(at)?;
                let mut cache = allocation.flat_boundaries.borrow_mut();
                let boundaries = cache.entry(at.offset).or_insert_with(|| vec![0]);
                if boundaries.len() > index + 1 {
                    let (start, end) = (boundaries[index], boundaries[index + 1]);
                    if u128::from(start) != offset {
                        return fail(
                            V::Malformed,
                            format!(
                                "sequential element {index} lies {start} bytes past the first, not {offset}"
                            ),
                        );
                    }
                    return Ok(Some((first.at(start), end)));
                }
                (boundaries.len(), *boundaries.last().unwrap())
            };
            self.ledger.counters.flat_prefix_reads += 1;
            let extent = self.bounded_size(first.at(last), element)?;
            let end = last
                .checked_add(extent)
                .ok_or_else(|| err(V::Malformed, "a sequential element extent overflows"))?;
            // Check every boundary against the actual enclosing extent.
            self.ledger
                .check(at)?
                .extent
                .check_range(first.offset + last, extent)?;
            let allocation = self.ledger.check(at)?;
            let mut cache = allocation.flat_boundaries.borrow_mut();
            let boundaries = cache.get_mut(&at.offset).unwrap();
            debug_assert_eq!(boundaries.len(), len);
            boundaries.push(end);
        }
    }

    /// H's own boundaries, derived from checked bytes, never from an IR offset.
    /// The allocation owns the cache: Ledger::get_mut drops every boundary
    /// before any possible write, including a same-allocation replacement.
    pub(super) fn flat_boundary(
        &mut self,
        layout: LayoutId,
        at: Ptr,
        index: u128,
    ) -> Checked<Option<(Ptr, LayoutId, u64)>> {
        if index >= u128::from(self.flat_count(layout, at)?) {
            return Ok(None);
        }
        let Some((16, element)) = self.flat_prefixed(layout)? else {
            return fail(V::TypeConfusion, "a flat walk of fixed elements");
        };
        let index = index as usize;
        loop {
            let (len, last) = {
                let allocation = self.ledger.check(at)?;
                let mut cache = allocation.flat_boundaries.borrow_mut();
                let boundaries = cache.entry(at.offset).or_insert_with(|| vec![0]);
                if boundaries.len() > index + 1 {
                    return Ok(Some((
                        at.at(16 + boundaries[index]),
                        element,
                        boundaries[index + 1],
                    )));
                }
                (boundaries.len(), *boundaries.last().unwrap())
            };
            self.ledger.counters.flat_prefix_reads += 1;
            let length = self.prefix_at(at.at(16 + last))?;
            let end = last
                .checked_add(PREFIX)
                .and_then(|v| v.checked_add(length))
                .ok_or_else(|| err(V::Malformed, "a flat element extent overflows"))?;
            // Check every boundary against the actual enclosing extent.
            self.ledger
                .check(at)?
                .extent
                .check_range(at.offset + 16 + last, PREFIX + length)?;
            let allocation = self.ledger.check(at)?;
            let mut cache = allocation.flat_boundaries.borrow_mut();
            let boundaries = cache.get_mut(&at.offset).unwrap();
            debug_assert_eq!(boundaries.len(), len);
            boundaries.push(end);
        }
    }

    /// The bytes a String or Bytes slice shows: its owner's payload from
    /// `8 + start` for `length` bytes (a sequence payload past its count, or a
    /// flat encoding past its prefix).
    pub(super) fn slice_bytes(&self, val: &Val, layout: LayoutId) -> Checked<Vec<u8>> {
        let Val::Image(image) = val else {
            return fail(V::Unsupported, format!("a slice held as {val:?}"));
        };
        self.slice_content(image, 0, layout)
    }

    /// The bytes the slice of `layout` stored at `at` in `image` shows.
    pub(super) fn slice_content(
        &self,
        image: &Extent,
        at: u64,
        layout: LayoutId,
    ) -> Checked<Vec<u8>> {
        let record = self.record(layout)?;
        let word = |ordinal: u32| {
            record
                .fields
                .iter()
                .find(|f| f.ordinal == ordinal)
                .map(|f| at + f.offset)
                .ok_or_else(|| err(V::Malformed, "a slice layout without its words"))
        };
        let owner = match image.cells.get(&word(0)?) {
            Some(Cell::Data { ptr, .. }) => *ptr,
            _ => return fail(V::ForgedPointer, "a slice owner is not a handle"),
        };
        let read = |offset: u64| -> Checked<u64> {
            image.check_range(offset, 8)?;
            let mut bytes = [0u8; 8];
            bytes.copy_from_slice(image.read_bytes(offset, 8));
            Ok(u64::from_le_bytes(bytes))
        };
        let (start, length) = (read(word(1)?)?, read(word(2)?)?);
        let from = owner.at(PREFIX + start);
        let a = self.ledger.check(from)?;
        a.extent.check_range(from.offset, length)?;
        if !a.extent.all_init(from.offset, length) {
            return fail(V::UninitializedRead, "a slice shows uninitialized bytes");
        }
        Ok(a.extent.read_bytes(from.offset, length).to_vec())
    }

    /// A slice image of `layout` over `owner`'s payload: start and length.
    pub(super) fn slice_image(
        &self,
        layout: LayoutId,
        owner: Ptr,
        start: u64,
        length: u64,
    ) -> Checked<Extent> {
        let record = self.record(layout)?;
        let mut image = Extent::new(self.size(layout)?);
        for f in &record.fields {
            match f.ordinal {
                0 => image.put_cell(
                    f.offset,
                    self.word,
                    Cell::Data {
                        ptr: owner,
                        counted: false,
                        borrow: None,
                    },
                ),
                1 => image.write_bytes(f.offset, &start.to_le_bytes()),
                2 => image.write_bytes(f.offset, &length.to_le_bytes()),
                _ => return fail(V::Malformed, "a slice layout with a fourth word"),
            }
        }
        Ok(image)
    }

    /// `FlatView`: the String or Bytes encoding at `r` as a slice of the block
    /// holding it, at the encoding's offset in that block's payload.
    pub(super) fn flat_view(&self, r: &Resolved, layout: LayoutId) -> Checked<Val> {
        let length = self.prefix_at(r.ptr)?;
        let owner = Ptr { offset: 0, ..r.ptr };
        Ok(Val::Image(self.slice_image(
            layout,
            owner,
            r.ptr.offset,
            length,
        )?))
    }

    /// The FlatFormatV0 bytes of one field value of a flat construction: a
    /// fixed field's value as stored, an encoded one behind its prefix.
    fn flat_field_bytes(
        &mut self,
        f: &mut Frame,
        fv: &FieldValue,
        fl: LayoutId,
    ) -> Checked<Vec<u8>> {
        let source = self.value_type(f.unit, fv.value)?.layout;
        if matches!(self.record(fl)?.size, SizeSpec::Fixed(_)) {
            let size = self.size(fl)?;
            let temp = self.temporary(fl)?;
            let m = self.take_value(f, fv.value)?;
            self.write_value(temp, fl, m)?;
            let e = self
                .ledger
                .get_mut(temp.alloc)
                .extent
                .take(0, size, FREE_POISON, None);
            self.ledger.free(temp.alloc)?;
            if !e.cells.is_empty() || !e.resources.is_empty() || !e.all_init(0, size) {
                return fail(
                    V::TypeConfusion,
                    "a flat field holds a handle or undefined bytes",
                );
            }
            return Ok(e.read_bytes(0, size).to_vec());
        }
        // An encoded source is only read (LIR schema revision 4).
        let val = self.read_use(f, fv.value)?;
        self.encode_flat_value(source, fl, &val)
    }

    /// Read and encode a source value without moving or duplicating its holders.
    /// Sequence elements are obtained from H's checked storage at the declared
    /// stride; a variable flat element contributes its own prefixed payload.
    fn encode_flat_value(
        &mut self,
        source: LayoutId,
        encoding: LayoutId,
        val: &Val,
    ) -> Checked<Vec<u8>> {
        if let SizeSpec::Fixed(size) = self.record(encoding)?.size
            && (source == encoding
                || matches!((&self.record(source)?.kind, &self.record(encoding)?.kind),
                (LayoutKind::Scalar(a), LayoutKind::Scalar(b)) if a == b))
        {
            return match val {
                Val::Unit if size == 0 => Ok(Vec::new()),
                Val::Bits { bits, .. } => Ok(bits.to_le_bytes()[..size as usize].to_vec()),
                Val::Image(image)
                    if image.cells.is_empty()
                        && image.resources.is_empty()
                        && image.all_init(0, size) =>
                {
                    Ok(image.read_bytes(0, size).to_vec())
                }
                _ => fail(
                    V::TypeConfusion,
                    "a fixed flat element is not initialized pointer-free bytes",
                ),
            };
        }
        let Some((offset, content)) = self.flat_prefixed(encoding)? else {
            return fail(V::TypeConfusion, "a variable flat encoding has no prefix");
        };
        let mut bytes = match self.record(source)?.kind {
            LayoutKind::Slice { .. } if offset == 8 => self.slice_bytes(val, source)?,
            LayoutKind::Block { payload } | LayoutKind::Box { payload } => {
                let Val::Ptr(p) = val else {
                    return fail(V::TypeConfusion, "a flat source is not a block");
                };
                if payload == content && offset == 8 {
                    let size = self.extent_of(content, *p)?;
                    let a = self.ledger.check(*p)?;
                    a.extent.check_range(p.offset, size)?;
                    if !a.extent.all_init(p.offset, size) {
                        return fail(
                            V::UninitializedRead,
                            "an encoded flat record is not initialized",
                        );
                    }
                    a.extent.read_bytes(p.offset, size).to_vec()
                } else if let LayoutKind::Sequential { element } = self.record(payload)?.kind {
                    // A flat list builder (LIR schema revision 22): its count, then
                    // its used bytes, which must be exactly `count` prefixed
                    // elements, each of its content's own extent.
                    if offset != 16 || element != content {
                        return fail(V::TypeConfusion, "a flat builder of another encoding");
                    }
                    let Some((8, record)) = self.flat_prefixed(element)? else {
                        return fail(V::TypeConfusion, "a flat builder of unprefixed elements");
                    };
                    let count = self.prefix_at(*p)?;
                    let used = self.prefix_at(p.at(8))?;
                    let start = p.at(16);
                    let a = self.ledger.check(start)?;
                    a.extent.check_range(start.offset, used)?;
                    if !a.extent.all_init(start.offset, used) {
                        return fail(V::UninitializedRead, "a flat builder's bytes");
                    }
                    let mut walked = 0u64;
                    for _ in 0..count {
                        let at = start.at(walked);
                        let len = self.prefix_at(at)?;
                        if self.extent_of(record, at.at(8))? != len {
                            return fail(V::Malformed, "a built element disagrees with its prefix");
                        }
                        walked = walked
                            .checked_add(8 + len)
                            .filter(|w| *w <= used)
                            .ok_or_else(|| {
                                err(V::Malformed, "built elements overrun the used bytes")
                            })?;
                    }
                    if walked != used {
                        return fail(
                            V::Malformed,
                            "a flat builder's used bytes are not its elements",
                        );
                    }
                    let mut bytes = count.to_le_bytes().to_vec();
                    bytes.extend_from_slice(
                        self.ledger
                            .check(start)?
                            .extent
                            .read_bytes(start.offset, used),
                    );
                    bytes
                } else if let LayoutKind::Sequence { element } = self.record(payload)?.kind {
                    let count = self.prefix_at(*p)?;
                    let stride = self.size(element)?;
                    let start = hirsel_lir::layout::sequence_elements_offset(self.record(element)?)
                        .map_err(|_| err(V::Malformed, "a flat source's element alignment"))?;
                    let extent = count
                        .checked_mul(stride)
                        .and_then(|n| n.checked_add(start))
                        .ok_or_else(|| {
                            err(V::Malformed, "a flat source sequence extent overflows")
                        })?;
                    self.ledger
                        .check(*p)?
                        .extent
                        .check_range(p.offset, extent)?;
                    let mut bytes = Vec::new();
                    if offset == 16 {
                        bytes.extend(count.to_le_bytes());
                    }
                    for index in 0..count {
                        let at = p.at(start + index * stride);
                        let a = self.ledger.check(at)?;
                        a.extent.check_range(at.offset, stride)?;
                        let image = a.extent.copy(at.offset, stride, Some((&[], &[])));
                        let repr = match self.record(element)?.kind {
                            LayoutKind::Scalar(k) => ValueRepr::Scalar(k),
                            LayoutKind::Block { .. } => ValueRepr::Block,
                            LayoutKind::Unit => ValueRepr::Unit,
                            _ => ValueRepr::Aggregate,
                        };
                        let val = self.val_from(image, repr, element)?.0;
                        bytes.extend(self.encode_flat_value(element, content, &val)?);
                    }
                    bytes
                } else {
                    return fail(V::TypeConfusion, "a flat field source has no encoding");
                }
            }
            _ => return fail(V::TypeConfusion, "a flat field source has no encoding"),
        };
        let mut out = (bytes.len() as u64).to_le_bytes().to_vec();
        out.append(&mut bytes);
        Ok(out)
    }

    /// A construction of the Flat record `layout`: its fields' FlatFormatV0
    /// bytes in declaration order, in a block sized to them (or inline when
    /// the record is fixed).
    pub(super) fn construct_flat(
        &mut self,
        f: &mut Frame,
        site: &ConstructSite,
        fields: &[FieldValue],
        block: bool,
    ) -> Checked<Val> {
        let bytes = self.flat_content(f, site, fields, None)?;
        let size = bytes.len() as u64;
        let target = if block {
            self.place_sized(f, site.placement, site.layout, size)?
        } else {
            if self.size(site.layout)? != size {
                return fail(V::Malformed, "a fixed flat construction of another size");
            }
            let mut image = Extent::new(size);
            image.write_bytes(0, &bytes);
            return Ok(Val::Image(image));
        };
        self.ledger
            .get_mut(target.alloc)
            .extent
            .write_bytes(target.offset, &bytes);
        if self.extent_of(site.layout, target)? != size {
            return fail(
                V::Malformed,
                "a flat construction disagrees with its size program",
            );
        }
        Ok(Val::Ptr(target))
    }

    /// The FlatFormatV0 bytes of the flat construction `site`: its fields'
    /// encodings in declaration order, checked against its size program.
    /// `open` is a field built in place and its encoding (LIR schema
    /// revision 24), for which the site has no value.
    pub(super) fn flat_content(
        &mut self,
        f: &mut Frame,
        site: &ConstructSite,
        fields: &[FieldValue],
        open: Option<(u32, Vec<u8>)>,
    ) -> Checked<Vec<u8>> {
        let record = self.record(site.layout)?;
        let mut declared: Vec<_> = record.fields.iter().collect();
        declared.sort_by_key(|fl| fl.ordinal);
        if declared.len() != fields.len() + usize::from(open.is_some()) {
            return fail(V::Malformed, "a flat construction misses a field");
        }
        let mut bytes = Vec::new();
        for fl in declared {
            if let Some((field, encoded)) = &open
                && fl.ordinal == *field
            {
                if fields.iter().any(|fv| fv.field == *field) {
                    return fail(V::Malformed, "a value for a field built in place");
                }
                bytes.extend_from_slice(encoded);
                continue;
            }
            let fv = fields
                .iter()
                .find(|fv| fv.field == fl.ordinal)
                .ok_or_else(|| {
                    err(
                        V::Malformed,
                        format!("no value for flat field {}", fl.ordinal),
                    )
                })?;
            bytes.extend(self.flat_field_bytes(f, fv, fl.layout)?);
        }
        let size = bytes.len() as u64;
        if hirsel_lir::layout::evaluate_size(&self.record(site.layout)?.size, &bytes).ok()
            != Some(size)
        {
            return fail(
                V::Malformed,
                "a flat construction disagrees with its size program",
            );
        }
        Ok(bytes)
    }
}

fn scalar_width(scalar: ScalarKind) -> Checked<u32> {
    Ok(match scalar {
        ScalarKind::U8 | ScalarKind::I8 | ScalarKind::Bool => 1,
        ScalarKind::U16 | ScalarKind::I16 => 2,
        ScalarKind::U32 | ScalarKind::I32 | ScalarKind::F32 => 4,
        ScalarKind::U64 | ScalarKind::I64 | ScalarKind::F64 => 8,
        ScalarKind::U128 | ScalarKind::I128 => 16,
        ScalarKind::Pointer => return fail(V::TypeConfusion, "a length stored as a pointer"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::build::Layouts;
    use crate::program::NoOperations;
    use hirsel_lir::layout_types::{FieldLayout, SizeTerm, TargetSpec};
    use hirsel_lir::stage::Rc;

    fn image(first: &[u8]) -> Vec<u8> {
        let mut bytes = vec![0; 48];
        bytes[..8].copy_from_slice(&40u64.to_le_bytes());
        bytes[8..16].copy_from_slice(&2u64.to_le_bytes());
        bytes[16..24].copy_from_slice(&(first.len() as u64).to_le_bytes());
        bytes[24..24 + first.len()].copy_from_slice(first);
        let second = 24 + first.len();
        bytes[second..second + 8].copy_from_slice(&1u64.to_le_bytes());
        bytes[second + 8] = b'z';
        bytes
    }

    #[test]
    fn a_headerless_slice_can_be_read_but_not_copied_as_an_owner() {
        let mut layouts = Layouts::new(TargetSpec::x86_64_unknown_linux_gnu());
        let byte = layouts.scalar(ScalarKind::U8);
        let pointer = layouts.scalar(ScalarKind::Pointer);
        let u64 = layouts.scalar(ScalarKind::U64);
        let slice = layouts.record(&[pointer, u64, u64]);
        let record = layouts.get_mut(slice);
        record.kind = LayoutKind::Slice { element: byte };
        record.scan = vec![hirsel_lir::layout_types::ScanStep::SliceOwner { offset: 0 }];
        record.copy = hirsel_lir::layout_types::CopyRecipe::Dup;
        let (target, table) = layouts.finish();
        let program = Program::<Rc> {
            target,
            layouts: table,
            units: vec![],
            statics: vec![],
        };
        let config = Config::default();
        let mut ops = NoOperations;
        let mut machine = Machine::new(&program, &config, &mut ops);
        let owner = machine
            .ledger
            .alloc(Owner::Temporary, None, 10, 10, 1, None);
        machine
            .ledger
            .get_mut(owner.alloc)
            .extent
            .write_bytes(0, &2u64.to_le_bytes());
        machine
            .ledger
            .get_mut(owner.alloc)
            .extent
            .write_bytes(8, b"ab");
        let view = Val::Image(machine.slice_image(slice, owner, 0, 2).unwrap());
        assert_eq!(machine.slice_bytes(&view, slice).unwrap(), b"ab");
        assert_eq!(
            machine.copy_value(view, slice).unwrap_err().kind,
            V::TypeConfusion
        );
    }

    #[test]
    fn cached_boundaries_reject_wrong_offsets_and_are_invalidated_by_same_allocation_writes() {
        let mut layouts = Layouts::new(TargetSpec::x86_64_unknown_linux_gnu());
        let byte = layouts.scalar(ScalarKind::U8);
        let mut prefix = |offset, child| {
            let id = layouts.record(&[]);
            let r = layouts.get_mut(id);
            r.kind = LayoutKind::Flat;
            r.size = SizeSpec::Dynamic(vec![
                SizeTerm::Constant(8),
                SizeTerm::ReadLength {
                    offset: 0,
                    scalar: ScalarKind::U64,
                },
                SizeTerm::Add,
            ]);
            r.fields = vec![FieldLayout {
                ordinal: 0,
                offset,
                layout: child,
                unaligned: false,
            }];
            id
        };
        let element = prefix(8, byte);
        let list = prefix(16, element);
        let (target, table) = layouts.finish();
        let program = Program::<Rc> {
            target,
            layouts: table,
            units: vec![],
            statics: vec![],
        };
        let config = Config::default();
        let mut ops = NoOperations;
        let mut m = Machine::new(&program, &config, &mut ops);
        let at = m
            .ledger
            .alloc(Owner::Heap, Some(list), 48, 48, 1, Some(Count::Finite(1)));
        m.ledger
            .get_mut(at.alloc)
            .extent
            .write_bytes(0, &image(b"ab"));
        assert!(m.flat_at(list, at, 0, 0).unwrap().is_some());
        assert!(m.flat_at(list, at, 1, 1).is_err());
        assert!(m.flat_at(list, at, 1, 10).unwrap().is_some());
        assert_eq!(m.ledger.counters.flat_prefix_reads, 2);
        // Keep allocation and generation unchanged; only its bytes change.
        m.ledger
            .get_mut(at.alloc)
            .extent
            .write_bytes(0, &image(b"abc"));
        assert!(m.flat_at(list, at, 1, 10).is_err());
        assert!(m.flat_at(list, at, 1, 11).unwrap().is_some());
        assert_eq!(m.ledger.counters.flat_prefix_reads, 4);
    }
}
