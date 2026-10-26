//! Closed typed subset of existing compiler value metadata. Unknown/opaque,
//! resource, handle and variable flat recipes refuse before transaction entry.
//! This follows handle_scan.rs's field/variant/sequence projection, rather than
//! inferring storage closure from a drop program.
use super::*;
use hirsel_core_contract::handle_scan::*;
use std::collections::BTreeSet;

pub(super) struct Edge {
    pub offset: usize,
    pub payload: Option<u32>,
    pub shared: bool,
    pub slice: bool,
}

pub(super) unsafe fn layout<'a>(ctx: *mut TaskCtx, index: u32) -> Result<&'a Layout, Error> {
    if index >= unsafe { (*ctx).heap_ref().layout_count() } {
        return Err(Error::Descriptor);
    }
    let layout = unsafe { &*(*ctx).descriptor(index) };
    if layout.magic != crate::LAYOUT_MAGIC
        || layout.version != crate::LAYOUT_VERSION
        || layout.finalizer.is_some()
        || !layout.align.is_power_of_two()
        || std::alloc::Layout::from_size_align(layout.size as usize, layout.align as usize).is_err()
        || layout.flags & (crate::LAYOUT_MUTEX_ROOT | crate::LAYOUT_UNSAFE_FFI) != 0
    {
        return Err(Error::Descriptor);
    }
    if layout.step_count != 0 {
        if layout.steps.is_null() {
            return Err(Error::Descriptor);
        }
        for step in unsafe { std::slice::from_raw_parts(layout.steps, layout.step_count as usize) }
        {
            if step.kind == crate::STEP_THAW && layout.flags & crate::LAYOUT_FROZEN != 0 {
                continue;
            }
            if !matches!(
                step.kind,
                crate::STEP_RELEASE
                    | crate::STEP_RELEASE_LAST
                    | crate::STEP_RELEASE_ROOT
                    | crate::STEP_TAG
                    | crate::STEP_NONNULL
                    | crate::STEP_LIVE_BIT
                    | crate::STEP_LOOP
                    | crate::STEP_COLUMN
            ) {
                return Err(Error::Descriptor);
            }
        }
    }
    Ok(layout)
}

fn range(offset: usize, size: usize, extent: usize) -> Result<(), Error> {
    if offset.checked_add(size).is_some_and(|end| end <= extent) {
        Ok(())
    } else {
        Err(Error::Descriptor)
    }
}

pub(super) unsafe fn edges(
    ctx: *mut TaskCtx,
    index: u32,
    source: *mut u8,
    extent: usize,
) -> Result<Vec<Edge>, Error> {
    unsafe { check_types(ctx, index) }?;
    let mut out = Vec::new();
    // One cursor per active inline type, not a recursive host call. Inline
    // descriptor cycles are invalid; ordinary block cycles cannot be expressed.
    let mut pending = vec![(index, 0usize, extent, BTreeSet::new())];
    while let Some((id, base, available, mut ancestors)) = pending.pop() {
        if !ancestors.insert(id) {
            return Err(Error::Descriptor);
        }
        let layout = unsafe { layout(ctx, id) }?;
        range(base, available, extent)?;
        if layout.size as usize > available {
            return Err(Error::Descriptor);
        }
        // Registry slot zero is the runtime's own closed byte-owner recipe,
        // predating value metadata. Its active payload is count/capacity/bytes;
        // it has no typed outgoing edge, independent of its empty cleanup.
        if id == 0 {
            range(0, 16, available)?;
            let count = unsafe { source.add(base).cast::<u64>().read_unaligned() };
            let capacity = unsafe { source.add(base + 8).cast::<u64>().read_unaligned() };
            if count > capacity || count.checked_add(16).is_none_or(|n| n > available as u64) {
                return Err(Error::Image);
            }
            continue;
        }
        if layout.scan_words == 0 {
            if !unsafe { super::super::guard::bits::pointer_free_descriptor(layout) } {
                return Err(Error::Descriptor);
            }
            continue;
        }
        if layout.scan_words == u64::MAX
            || layout.scan_words % WORDS as u64 != 0
            || layout.scan.is_null()
            || !layout.scan.addr().is_multiple_of(align_of::<u64>())
            || layout.scan_words > isize::MAX as u64 / 8
        {
            return Err(Error::Descriptor);
        }
        let ops: Vec<_> =
            unsafe { std::slice::from_raw_parts(layout.scan, layout.scan_words as usize) }
                .chunks_exact(WORDS)
                .collect();
        // Validate the entire program, including inactive arms and type-only
        // edges. Dynamic authority/unknown descriptors cannot hide in an arm.
        for op in &ops {
            if !matches!(
                op[0],
                BLOCK
                    | ROOT
                    | INLINE
                    | TAG
                    | NONNULL
                    | LIVE_BIT
                    | SEQUENCE
                    | REPEAT
                    | TYPE_EDGE
                    | VALUE
                    | VALUE_FIELD
            ) {
                return Err(Error::Descriptor);
            }
        }
        let mut values = ops.iter().filter(|op| op[0] == VALUE);
        let value = values.next().ok_or(Error::Descriptor)?;
        if values.next().is_some() || value[2] != layout.size || value[7] != 0 {
            return Err(Error::Descriptor);
        }
        let fields: Vec<_> = ops.iter().filter(|op| op[0] == VALUE_FIELD).collect();
        let mut push_field = |op: &[u64]| -> Result<(), Error> {
            if op[4..] != [0; 4] {
                return Err(Error::Descriptor);
            }
            let offset = usize::try_from(op[1]).map_err(|_| Error::Descriptor)?;
            let child = u32::try_from(op[2]).map_err(|_| Error::Descriptor)?;
            let child_layout = unsafe { self::layout(ctx, child) }?;
            if child_layout.flags & crate::LAYOUT_DYNAMIC != 0 {
                return Err(Error::Descriptor);
            }
            let size = child_layout.size as usize;
            range(offset, size, available)?;
            pending.push((child, base + offset, size, ancestors.clone()));
            Ok(())
        };
        match value[1] {
            VALUE_BITS | VALUE_BOOL => {
                if !fields.is_empty()
                    || value[3..] != [0; 5]
                    || (value[1] == VALUE_BITS && !matches!(layout.size, 1 | 2 | 4 | 8 | 16))
                    || (value[1] == VALUE_BOOL && layout.size != 1)
                    || ops.iter().any(|op| !matches!(op[0], VALUE | TYPE_EDGE))
                {
                    return Err(Error::Descriptor);
                }
            }
            VALUE_FIELDS => {
                if value[3..] != [0; 5] || fields.len() != layout.field_count as usize {
                    return Err(Error::Descriptor);
                }
                let mut intervals = Vec::new();
                for field in fields {
                    if field[3] != u64::MAX {
                        return Err(Error::Descriptor);
                    }
                    let child = unsafe {
                        self::layout(ctx, u32::try_from(field[2]).map_err(|_| Error::Descriptor)?)
                    }?;
                    intervals.push((
                        field[1],
                        field[1].checked_add(child.size).ok_or(Error::Descriptor)?,
                    ));
                    push_field(field)?;
                }
                intervals.sort_unstable();
                if intervals.windows(2).any(|pair| pair[0].1 > pair[1].0) {
                    return Err(Error::Descriptor);
                }
            }
            VALUE_VARIANT => {
                let offset = usize::try_from(value[3]).map_err(|_| Error::Descriptor)?;
                let arm = match value[4] {
                    0 => {
                        range(offset, 4, available)?;
                        unsafe {
                            u64::from(source.add(base + offset).cast::<u32>().read_unaligned())
                        }
                    }
                    1 if value[5] < 2 => {
                        range(offset, 8, available)?;
                        let null = unsafe {
                            source
                                .add(base + offset)
                                .cast::<*const u8>()
                                .read_unaligned()
                                .is_null()
                        };
                        if null { value[5] } else { 1 - value[5] }
                    }
                    _ => return Err(Error::Descriptor),
                };
                if arm >= value[6] {
                    return Err(Error::Image);
                }
                for field in fields {
                    if field[3] >= value[6] {
                        return Err(Error::Descriptor);
                    }
                    if field[3] == arm {
                        push_field(field)?;
                    }
                }
            }
            VALUE_INDIRECT | VALUE_SLICE => {
                let slice = value[1] == VALUE_SLICE;
                let size = if slice { 24 } else { 8 };
                if layout.size != size || !fields.is_empty() || value[4..] != [0; 4] {
                    return Err(Error::Descriptor);
                }
                let child = u32::try_from(value[3]).map_err(|_| Error::Descriptor)?;
                let child_layout = unsafe { self::layout(ctx, child) }?;
                if slice
                    && (child_layout.size != 1
                        || !unsafe {
                            super::super::guard::bits::pointer_free_descriptor(child_layout)
                        })
                {
                    return Err(Error::Descriptor);
                }
                let holders: Vec<_> = ops
                    .iter()
                    .filter(|op| matches!(op[0], BLOCK | ROOT))
                    .collect();
                if holders.len() != 1 || holders[0][1..] != [0; 7] {
                    return Err(Error::Descriptor);
                }
                out.push(Edge {
                    offset: base,
                    payload: (!slice).then_some(child),
                    shared: holders[0][0] == ROOT,
                    slice,
                });
            }
            VALUE_SEQUENCE => {
                if layout.flags & crate::LAYOUT_DYNAMIC == 0
                    || value[2] != 16
                    || value[6..] != [0; 2]
                    || !fields.is_empty()
                {
                    return Err(Error::Descriptor);
                }
                range(0, 16, available)?;
                let count = unsafe { source.add(base).cast::<u64>().read_unaligned() };
                let capacity = unsafe { source.add(base + 8).cast::<u64>().read_unaligned() };
                let stride = value[3];
                let start = value[4];
                let child = u32::try_from(value[5]).map_err(|_| Error::Descriptor)?;
                let child_layout = unsafe { self::layout(ctx, child) }?;
                if count > capacity
                    || child_layout.flags & crate::LAYOUT_DYNAMIC != 0
                    || child_layout.size > stride
                    || start < 16
                    || count
                        .checked_mul(stride)
                        .and_then(|n| n.checked_add(start))
                        .is_none_or(|n| n > available as u64)
                {
                    return Err(Error::Image);
                }
                if stride == 0 {
                    // A sequence of Unit can have any length without owning
                    // an element slot. Do not allocate work per logical unit.
                    if child_layout.size != 0
                        || !unsafe {
                            super::super::guard::bits::pointer_free_descriptor(child_layout)
                        }
                    {
                        return Err(Error::Descriptor);
                    }
                    continue;
                }
                for i in 0..count {
                    pending.push((
                        child,
                        base + (start + i * stride) as usize,
                        child_layout.size as usize,
                        ancestors.clone(),
                    ));
                }
            }
            _ => return Err(Error::Descriptor),
        }
    }
    out.sort_by_key(|edge| edge.offset);
    if out
        .windows(2)
        .any(|pair| pair[0].offset + if pair[0].slice { 24 } else { 8 } > pair[1].offset)
    {
        return Err(Error::Descriptor);
    }
    let scanned = unsafe { scanned_edges(ctx, index, source, extent) }?;
    if scanned
        != out
            .iter()
            .map(|edge| (edge.offset, edge.shared))
            .collect::<Vec<_>>()
    {
        return Err(Error::Coverage);
    }
    Ok(out)
}

unsafe fn check_types(ctx: *mut TaskCtx, root: u32) -> Result<(), Error> {
    let mut pending = vec![root];
    let mut seen = BTreeSet::new();
    while let Some(id) = pending.pop() {
        if !seen.insert(id) {
            continue;
        }
        let layout = unsafe { layout(ctx, id) }?;
        if id == 0 {
            continue;
        }
        if layout.scan_words == 0 {
            if !unsafe { super::super::guard::bits::pointer_free_descriptor(layout) } {
                return Err(Error::Descriptor);
            }
            continue;
        }
        if layout.scan_words == u64::MAX
            || layout.scan_words % WORDS as u64 != 0
            || layout.scan.is_null()
            || !layout.scan.addr().is_multiple_of(align_of::<u64>())
            || layout.scan_words > isize::MAX as u64 / 8
        {
            return Err(Error::Descriptor);
        }
        let ops = unsafe { std::slice::from_raw_parts(layout.scan, layout.scan_words as usize) };
        let mut values = 0;
        for op in ops.chunks_exact(WORDS) {
            let linked = match op[0] {
                INLINE | VALUE_FIELD => Some(op[2]),
                TYPE_EDGE => {
                    // These include representation-only pointer scalars, not
                    // owned payload values (e.g. a List holder's carrier).
                    if op[2] >= unsafe { (*ctx).heap_ref().layout_count() } as u64 {
                        return Err(Error::Descriptor);
                    }
                    None
                }
                SEQUENCE | REPEAT => Some(op[4]),
                VALUE => {
                    values += 1;
                    if op[2] != layout.size || op[7] != 0 {
                        return Err(Error::Descriptor);
                    }
                    match op[1] {
                        VALUE_BITS | VALUE_BOOL | VALUE_FIELDS | VALUE_VARIANT => None,
                        VALUE_INDIRECT | VALUE_SLICE => Some(op[3]),
                        VALUE_SEQUENCE => Some(op[5]),
                        _ => return Err(Error::Descriptor),
                    }
                }
                BLOCK | ROOT | TAG | NONNULL | LIVE_BIT => None,
                _ => return Err(Error::Descriptor),
            };
            if let Some(linked) = linked {
                pending.push(u32::try_from(linked).map_err(|_| Error::Descriptor)?);
            }
        }
        if values != 1 {
            return Err(Error::Descriptor);
        }
    }
    Ok(())
}

/// Independent holder projection, following ordered/capture.rs's flat program
/// traversal. Compare it with the value recipe so omitted or extra typed edges
/// cannot be accepted merely because a cleanup program happens to balance.
unsafe fn scanned_edges(
    ctx: *mut TaskCtx,
    index: u32,
    source: *mut u8,
    extent: usize,
) -> Result<Vec<(usize, bool)>, Error> {
    let mut found = Vec::new();
    let mut pending = vec![(index, 0usize, extent, BTreeSet::new())];
    while let Some((id, base, available, mut ancestors)) = pending.pop() {
        if !ancestors.insert(id) {
            return Err(Error::Descriptor);
        }
        let layout = unsafe { layout(ctx, id) }?;
        if layout.scan_words == 0 {
            continue;
        }
        if layout.scan_words == u64::MAX
            || layout.scan_words % WORDS as u64 != 0
            || layout.scan.is_null()
        {
            return Err(Error::Descriptor);
        }
        let words = unsafe { std::slice::from_raw_parts(layout.scan, layout.scan_words as usize) };
        let mut pc = 0;
        while pc < words.len() {
            let op = &words[pc..pc + WORDS];
            pc += WORDS;
            let offset = usize::try_from(op[1]).map_err(|_| Error::Descriptor)?;
            match op[0] {
                BLOCK | ROOT => {
                    range(offset, 8, available)?;
                    found.push((base + offset, op[0] == ROOT));
                }
                INLINE => {
                    let child = u32::try_from(op[2]).map_err(|_| Error::Descriptor)?;
                    let size = unsafe { self::layout(ctx, child) }?.size as usize;
                    range(offset, size, available)?;
                    pending.push((child, base + offset, size, ancestors.clone()));
                }
                TAG | NONNULL | LIVE_BIT => {
                    let size = if op[0] == TAG { 4 } else { 8 };
                    range(offset, size, available)?;
                    let at = unsafe { source.add(base + offset) };
                    let active = match op[0] {
                        TAG => unsafe { u64::from(at.cast::<u32>().read_unaligned()) == op[2] },
                        NONNULL => unsafe { !at.cast::<*const u8>().read_unaligned().is_null() },
                        LIVE_BIT if op[4] < 64 => unsafe {
                            (at.cast::<u64>().read_unaligned() >> op[4]) & 1 == op[2]
                        },
                        _ => return Err(Error::Descriptor),
                    };
                    let span = usize::try_from(op[3]).map_err(|_| Error::Descriptor)?;
                    if !span.is_multiple_of(WORDS) || span > words.len() - pc {
                        return Err(Error::Descriptor);
                    }
                    if !active {
                        pc += span;
                    }
                }
                SEQUENCE | REPEAT => {
                    let count = if op[0] == REPEAT {
                        op[2]
                    } else {
                        let count_at = usize::try_from(op[2]).map_err(|_| Error::Descriptor)?;
                        range(count_at, 8, available)?;
                        unsafe { source.add(base + count_at).cast::<u64>().read_unaligned() }
                    };
                    let child = u32::try_from(op[4]).map_err(|_| Error::Descriptor)?;
                    let size = unsafe { self::layout(ctx, child) }?.size as usize;
                    if count != 0 {
                        let end = (count - 1)
                            .checked_mul(op[3])
                            .and_then(|n| n.checked_add(op[1]))
                            .ok_or(Error::Descriptor)?;
                        range(
                            usize::try_from(end).map_err(|_| Error::Descriptor)?,
                            size,
                            available,
                        )?;
                    }
                    for i in 0..count {
                        pending.push((
                            child,
                            base + offset + (i * op[3]) as usize,
                            size,
                            ancestors.clone(),
                        ));
                    }
                }
                TYPE_EDGE | VALUE | VALUE_FIELD => {}
                _ => return Err(Error::Descriptor),
            }
        }
    }
    found.sort_unstable();
    Ok(found)
}
