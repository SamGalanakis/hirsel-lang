//! `LayoutRecord` drop programs lowered to `hirsel_rt_v2` layout descriptors.
//!
//! The runtime's release engine walks one flat step program per descriptor and
//! keeps its whole walk state in a dead header's count word, so it never
//! allocates and uses constant native stack. This module is the only producer
//! of that program: emission writes the descriptors as static data and the
//! runtime's tests drive release from them. It reads the layout table and
//! computes nothing the table does not already say.
//!
//! The lowering is structural:
//! - `Release` and `Environment` release a counted holder (`STEP_RELEASE`);
//!   `CloseHandle` closes a run-context handle. The release that ends the
//!   program, through the arms of a last `ActiveVariant`, is a
//!   `STEP_RELEASE_LAST` (`mark_last`): the runtime frees its value before
//!   draining the child it kills.
//! - `Child` inlines the child's program at its offset; the child's leading
//!   `Finalize` becomes a `STEP_FINALIZE` naming the child's layout.
//! - `ActiveVariant` becomes one `STEP_TAG` per variant, whose span skips that
//!   variant's steps unless its ordinal is active. Under a null-pointer niche
//!   only the dataful variant has steps, and its arm is a `STEP_NONNULL` on
//!   the niche word.
//! - `ElementsReverse` and `RepeatReverse` become a `STEP_LOOP` (dynamic or
//!   static count) over the element's program; `ColumnsReverse` becomes a
//!   dynamic `STEP_LOOP` of stride 0 holding one `STEP_COLUMN` per dropped
//!   column, in reverse declaration order.
//! - `Thaw` (a `Shared` root's `Frozen` payload) becomes a `STEP_THAW` naming the
//!   frozen value's layout, so that layout gets its own descriptor too.
//! - The layout's own leading `Finalize` is the descriptor's finalizer.
//!
//! A dynamic count belongs only to an outermost loop: a dynamic extent is
//! never inline in another value, and the runtime's index encoding relies on
//! it. Each top-level step names the live-mask field it belongs to, which is
//! the field ordinal of the record, or of the active variant. The partition
//! replays the derivation's rule: a field with a finalizer contributes one
//! `Child` step, and any other field contributes its own ordered drop steps.
//! An absent contribution requires independent borrowed-scan evidence (a closure
//! environment's borrowed or `mut` capture, which it never drops). Byte ranges
//! never identify contributions: zero-sized siblings still have separate ordinals.
use crate::layout::{borrowed_scan, shift_drop};
use crate::layout_types::{
    DropStep, FieldLayout, FinalizerRef, LayoutId, LayoutKind, LayoutRecord, LayoutTable, ScanStep,
    SizeSpec, TagForm,
};
use crate::runtime_abi::{
    LAYOUT_DYNAMIC, LAYOUT_NO_EXTENT, LAYOUT_UNSAFE_FFI, NO_OFFSET, STEP_CLOSE_HANDLE, STEP_COLUMN,
    STEP_CURSOR_BITS, STEP_FINALIZE, STEP_LIVE_BIT, STEP_LOOP, STEP_NONNULL, STEP_RELEASE,
    STEP_RELEASE_LAST, STEP_RELEASE_RECEIPT, STEP_RELEASE_ROOT, STEP_TAG, STEP_THAW, WHOLE_VALUE,
};

/// Whether `record`'s arena blocks omit the extent before their header ([`LAYOUT_NO_EXTENT`]):
/// a fixed size that is not flat, so its capacity is its size. Emission and the runtime's
/// descriptor agree through this predicate, and the system-allocator archive makes exactly
/// these layouts' blocks bare (one malloc, no metadata). A sequence's capacity is a per-block
/// fact that compiled code reads from the block's metadata, so a sequence payload is never
/// one of them, whatever its size.
pub fn no_extent(record: &LayoutRecord) -> bool {
    matches!(record.size, SizeSpec::Fixed(_))
        && !matches!(
            record.kind,
            LayoutKind::Flat | LayoutKind::Sequence { .. } | LayoutKind::Sequential { .. }
        )
}

/// One runtime step kind (`HIRSEL_RT_V2_STEP_*`).
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum StepKind {
    /// Release the counted holder at `offset`.
    Release,
    /// A `Release` after which the value runs nothing more (see [`mark_last`]).
    ReleaseLast,
    /// Run `layout`'s finalizer on the inline value at `offset`, shielded.
    Finalize,
    /// End the finalizer's scope after its fields.
    EndFinalize,
    /// Close the run-context handle stored at `offset`.
    CloseHandle,
    /// Unless the u32 tag at `offset` equals `variant`, skip the next `span` steps.
    Tag,
    /// Unless the pointer word at `offset` is non-null, skip the next `span` steps.
    NonNull,
    /// Keep the span only when bit `variant` of the u64 live word is set.
    LiveBit,
    /// Run the next `span` steps once per element, highest index first.
    Loop,
    /// Run the next `span` steps once on the enclosing loop's element of one
    /// structure-of-arrays column.
    Column,
    /// Restore ordinary counts in the frozen value at `offset`, of `layout`.
    Thaw,
    /// Release the `Shared` root at `offset` under the root count contract; never a tail
    /// release.
    ReleaseRoot,
    /// Consume a private semantic metadata holder without language ownership effects.
    ReleaseReceipt,
}

impl StepKind {
    pub const fn code(self) -> u32 {
        match self {
            Self::Release => STEP_RELEASE,
            Self::ReleaseLast => STEP_RELEASE_LAST,
            Self::Finalize => STEP_FINALIZE,
            Self::EndFinalize => crate::runtime_abi::STEP_END_FINALIZE,
            Self::CloseHandle => STEP_CLOSE_HANDLE,
            Self::Tag => STEP_TAG,
            Self::NonNull => STEP_NONNULL,
            Self::LiveBit => STEP_LIVE_BIT,
            Self::Loop => STEP_LOOP,
            Self::Column => STEP_COLUMN,
            Self::Thaw => STEP_THAW,
            Self::ReleaseRoot => STEP_RELEASE_ROOT,
            Self::ReleaseReceipt => STEP_RELEASE_RECEIPT,
        }
    }
}

/// One `hirsel_rt_v2_step`, with `layout` as a table id that emission resolves
/// to its descriptor symbol. Offsets are from the enclosing frame's base: the
/// value, a loop's element or a column's element.
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct Step {
    pub kind: StepKind,
    /// Top-level field ordinal for a live mask, or `WHOLE_VALUE`.
    pub field: u32,
    /// `Tag`: the ordinal that keeps the span.
    pub variant: u32,
    /// `Tag`, `NonNull`, `Loop`, `Column`: the steps that follow and belong to this one.
    pub span: u32,
    pub offset: u64,
    /// `Loop`, `Column`: bytes between consecutive elements. On a release, 24
    /// identifies a String/Bytes slice triple (descriptor version 15).
    pub stride: u64,
    /// `Loop`: the static element count. `Column`: the bytes per element of
    /// the columns before it, so the column starts `capacity * count` in.
    pub count: u64,
    /// `Loop`: the offset of the dynamic u64 element count, or `NO_OFFSET` for
    /// a static count. `Column`: the offset of the u64 element capacity.
    pub count_offset: u64,
    /// `Finalize`: the inline resource's layout. `Thaw`: the frozen value's.
    pub layout: Option<LayoutId>,
}

/// One `hirsel_rt_v2_layout`.
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct Descriptor {
    pub layout: LayoutId,
    /// The fixed size, or for a dynamic extent the bytes every value has.
    pub size: u64,
    pub align: u64,
    /// The layout's own finalizer, run before its steps.
    pub finalizer: Option<FinalizerRef>,
    pub field_count: u32,
    pub flags: u32,
    /// The MEM-6 charge of a block of this payload layout (descriptor version 6), per
    /// element with `LAYOUT_LOGICAL_PER_ELEMENT`; `LOGICAL_UNSUPPORTED` without one.
    pub logical: u64,
    /// Optional native count program for live private entries.
    pub logical_count: Option<[u64; 5]>,
    pub steps: Vec<Step>,
    /// Holder scan, distinct from the destructor program.
    pub scan: crate::handle_scan::Program,
}

impl Descriptor {
    /// Layouts this descriptor's `Finalize` and `Thaw` steps name; each needs its own descriptor.
    pub fn referenced(&self) -> impl Iterator<Item = LayoutId> + '_ {
        self.steps.iter().filter_map(|s| s.layout)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DescriptorError {
    MissingLayout(LayoutId),
    /// A drop program the derivation never produces.
    Malformed(String),
    /// The program exceeds what a dead header's cursor can count.
    TooLarge(String),
}

fn malformed<T>(what: impl Into<String>) -> Result<T, DescriptorError> {
    Err(DescriptorError::Malformed(what.into()))
}

impl Descriptor {
    /// Reconstruct every static user-finalizer scope, including skipped arms
    /// and zero-count loops. Balanced totals alone cannot prove field coverage.
    pub fn check_finalizers(&self, table: &LayoutTable) -> Result<(), DescriptorError> {
        fn span(
            steps: &[Step],
            start: usize,
            end: usize,
            mut active: Vec<(LayoutId, u64, u32)>,
        ) -> Result<Vec<(LayoutId, u64, u32)>, DescriptorError> {
            let mut at = start;
            while at < end {
                let step = &steps[at];
                match step.kind {
                    StepKind::Tag
                    | StepKind::NonNull
                    | StepKind::LiveBit
                    | StepKind::Loop
                    | StepKind::Column => {
                        let close = at.checked_add(1 + step.span as usize).ok_or_else(|| {
                            DescriptorError::Malformed("finalizer scope span overflow".into())
                        })?;
                        if close > end {
                            return malformed("finalizer scope span leaves its field program");
                        }
                        if span(steps, at + 1, close, active.clone())? != active {
                            return malformed(
                                "conditional field cleanup changes its outer finalizer scopes",
                            );
                        }
                        at = close;
                        continue;
                    }
                    StepKind::Finalize => active.push((
                        step.layout.ok_or_else(|| {
                            DescriptorError::Malformed("finalizer entry lacks layout".into())
                        })?,
                        step.offset,
                        step.field,
                    )),
                    StepKind::EndFinalize => {
                        if active.pop() != step.layout.map(|id| (id, step.offset, step.field)) {
                            return malformed("finalizer scope end does not match its entry");
                        }
                    }
                    _ => (),
                }
                at += 1;
            }
            Ok(active)
        }
        // Count scope identities from the independent semantic drop programs.
        // This rejects deletion of a complete balanced begin/end pair too.
        let mut expected = std::collections::BTreeMap::new();
        let mut pending = vec![(
            self.layout,
            lookup(table, self.layout)?.drop.clone(),
            vec![self.layout],
        )];
        while let Some((owner, program, ancestry)) = pending.pop() {
            for step in program {
                let children = match step {
                    DropStep::Finalize => {
                        *expected.entry(owner).or_insert(0usize) += 1;
                        vec![]
                    }
                    DropStep::Child { layout, .. } => vec![layout],
                    DropStep::ElementsReverse { element, .. }
                    | DropStep::RepeatReverse { element, .. } => vec![element],
                    DropStep::ColumnsReverse(columns) => {
                        columns.columns.iter().map(|c| c.layout).collect()
                    }
                    DropStep::ActiveVariant { variants, .. } => {
                        for (_, arm) in variants {
                            pending.push((owner, arm, ancestry.clone()));
                        }
                        vec![]
                    }
                    _ => vec![],
                };
                for child in children {
                    if ancestry.contains(&child) {
                        return malformed("inline finalizer scope cycle");
                    }
                    let mut chain = ancestry.clone();
                    chain.push(child);
                    pending.push((child, lookup(table, child)?.drop.clone(), chain));
                }
            }
        }
        let own = lookup(table, self.layout)?.finalizer.is_some();
        let mut entered = std::collections::BTreeMap::new();
        if own {
            entered.insert(self.layout, 1usize);
        }
        let mut ended = std::collections::BTreeMap::new();
        for step in &self.steps {
            let counts = match step.kind {
                StepKind::Finalize => &mut entered,
                StepKind::EndFinalize => &mut ended,
                _ => continue,
            };
            let id = step
                .layout
                .ok_or_else(|| DescriptorError::Malformed("finalizer scope lacks layout".into()))?;
            *counts.entry(id).or_insert(0usize) += 1;
        }
        if entered != expected || ended != expected {
            return malformed("finalizer scopes do not cover the semantic field drop programs");
        }

        let initial = if own {
            vec![(self.layout, 0, WHOLE_VALUE)]
        } else {
            vec![]
        };
        if !span(&self.steps, 0, self.steps.len(), initial)?.is_empty() {
            return malformed("finalizer remains active after its field program");
        }
        Ok(())
    }
}

/// Implementation-only library drop thunks have reserved, validated $native identities.
fn language_finalizer(record: &LayoutRecord) -> bool {
    record.finalizer.as_ref().is_some_and(|f| {
        !f.definition
            .0
            .modules
            .iter()
            .any(|module| module.as_str() == "$native")
    })
}

/// The descriptor of one layout.
pub fn descriptor(table: &LayoutTable, id: LayoutId) -> Result<Descriptor, DescriptorError> {
    let storage = crate::logical_storage::MapStorage::new(table);
    descriptor_with_storage(table, id, &storage)
}
fn descriptor_with_storage(
    table: &LayoutTable,
    id: LayoutId,
    storage: &crate::logical_storage::MapStorage,
) -> Result<Descriptor, DescriptorError> {
    let record = lookup(table, id)?;
    let mut drop = record.drop.as_slice();
    if let Some(DropStep::Finalize) = drop.first() {
        if record.finalizer.is_none() {
            return malformed("a Finalize step in a layout without a finalizer");
        }
        drop = &drop[1..];
    } else if record.finalizer.is_some() {
        return malformed("a layout with a finalizer whose drop program does not start with it");
    }
    let mut lower = Lower {
        table,
        steps: Vec::new(),
        loops: 0,
        scope: (id, 0),
        arms: Vec::new(),
    };
    let field_count = match &record.tag {
        Some(tag) => {
            match drop {
                [] => {}
                [
                    DropStep::ActiveVariant {
                        tag_offset,
                        form,
                        variants,
                    },
                ] if *tag_offset == tag.offset && *form == tag.form => {
                    for (ordinal, steps) in variants {
                        let Some(variant) = tag.variants.iter().find(|v| v.ordinal == *ordinal)
                        else {
                            return malformed(format!(
                                "a drop program for absent variant {ordinal}"
                            ));
                        };
                        let at = lower.arm(tag.form, *ordinal, tag.offset)?;
                        lower.arms.push((tag.offset, *ordinal));
                        lower.fields(&variant.fields, steps, &record.scan, None)?;
                        lower.arms.pop();
                        lower.close(at)?;
                    }
                }
                _ => return malformed("a tagged layout's drop program is not one ActiveVariant"),
            }
            tag.variants
                .iter()
                .map(|v| field_count(&v.fields))
                .max()
                .unwrap_or(0)
        }
        None if matches!(record.kind, LayoutKind::Environment { .. }) => {
            let LayoutKind::Environment { movable } = &record.kind else {
                unreachable!()
            };
            let word = record.fields.last().ok_or_else(|| {
                DescriptorError::Malformed("environment without live word".into())
            })?;
            lower.fields(
                &record.fields,
                drop,
                &record.scan,
                Some((word.offset, movable)),
            )?;
            field_count(&record.fields)
        }
        None if matches!(record.kind, LayoutKind::Aggregate | LayoutKind::Resource) => {
            lower.fields(&record.fields, drop, &record.scan, None)?;
            field_count(&record.fields)
        }
        None => {
            lower.program(drop, 0)?;
            0
        }
    };
    let (size, dynamic) = match record.size {
        SizeSpec::Fixed(size) => (size, false),
        SizeSpec::Dynamic(_) => (minimum_extent(table, record)?, true),
    };
    let unsafe_ffi = record
        .finalizer
        .as_ref()
        .is_some_and(|f| f.unsafe_ffi_package);
    if record.finalizer.is_some() {
        let end = lower.push(StepKind::EndFinalize, 0);
        lower.steps[end].variant = 1; // own root; bypassed under a partial live mask
        lower.steps[end].layout = Some(id);
    }
    let end = lower.steps.len();
    mark_last(&mut lower.steps, 0, end);
    if lower.steps.len() as u64 >= 1 << STEP_CURSOR_BITS {
        return Err(DescriptorError::TooLarge(format!(
            "{} steps; a dead header's cursor counts fewer than 2^{STEP_CURSOR_BITS}",
            lower.steps.len()
        )));
    }
    // A Map's or Set's payload holds its entries at offset 0 (`LAYOUT_KEYED`).
    let keyed = crate::layout::keyed(table, id);
    if keyed
        && !record
            .fields
            .iter()
            .any(|f| f.ordinal == 0 && f.offset == 0)
    {
        return malformed("a Map or Set payload whose entries are not at offset 0");
    }
    let (logical, per_element) = match crate::layout::block_charge(table, id) {
        Some(crate::layout::LogicalCharge::Fixed(n)) => (n, 0),
        Some(crate::layout::LogicalCharge::PerElement(n)) => {
            (n, crate::runtime_abi::LAYOUT_LOGICAL_PER_ELEMENT)
        }
        None => (crate::runtime_abi::LOGICAL_UNSUPPORTED, 0),
    };
    let (logical, logical_count) = match storage.entries(id) {
        Some(entries) => (entries.logical, Some(entries.program())),
        None => (logical, None),
    };
    let descriptor = Descriptor {
        scan: crate::handle_scan::Program::lower_in(table, record),
        logical_count,
        layout: id,
        size,
        align: u64::from(record.align),
        finalizer: record.finalizer.clone(),
        field_count,
        logical,
        flags: per_element
            | if unsafe_ffi { LAYOUT_UNSAFE_FFI } else { 0 }
            | if record.finalizer.is_some() && !language_finalizer(record) {
                crate::runtime_abi::LAYOUT_IMPLEMENTATION_FINALIZER
            } else {
                0
            }
            | if dynamic { LAYOUT_DYNAMIC } else { 0 }
            | if no_extent(record) {
                LAYOUT_NO_EXTENT
            } else {
                0
            }
            | if matches!(record.kind, LayoutKind::Frozen { .. }) {
                crate::runtime_abi::LAYOUT_FROZEN
            } else {
                0
            }
            | if matches!(record.kind, LayoutKind::Frozen { value }
                if matches!(table.layouts[value.0 as usize].kind, LayoutKind::Handle | LayoutKind::HostCapability { .. }))
            {
                crate::runtime_abi::LAYOUT_MUTEX_ROOT
            } else {
                0
            }
            | if matches!(record.kind, LayoutKind::Sequence { .. }) {
                crate::runtime_abi::LAYOUT_SEQUENCE
            } else {
                0
            }
            | if matches!(record.kind, LayoutKind::SoaColumns { .. }) {
                crate::runtime_abi::LAYOUT_COLUMNS
            } else {
                0
            }
            | if matches!(record.kind, LayoutKind::Sequential { .. }) {
                crate::runtime_abi::LAYOUT_SEQUENTIAL
            } else {
                0
            }
            | if keyed {
                crate::runtime_abi::LAYOUT_KEYED
            } else if matches!(
                record.kind,
                LayoutKind::Opaque { .. } | LayoutKind::Environment { .. }
            ) {
                crate::runtime_abi::LAYOUT_UNTRACKED_BELOW
            } else {
                0
            }
            | if lower
                .steps
                .iter()
                .any(|s| matches!(s.kind, StepKind::Loop | StepKind::Column))
            {
                crate::runtime_abi::LAYOUT_WIDE_CURSOR
            } else {
                0
            },
        steps: lower.steps,
    };
    descriptor.check_finalizers(table)?;
    Ok(descriptor)
}

/// Whether a release or reset of a block of `id` under the static live mask `words` runs
/// nothing but its count edge and free: the layout has no finalizer (a masked root runs none
/// anyway, but its children's would) and every top-level step of its program belongs to a
/// field the mask leaves dead, as the runtime walk skips it (`WHOLE_VALUE` is always live).
/// Compiled code then resets or releases such a block without the runtime's walk.
pub fn masked_inert(
    table: &LayoutTable,
    id: LayoutId,
    words: &[u64],
) -> Result<bool, DescriptorError> {
    let d = descriptor(table, id)?;
    if d.finalizer.is_some() {
        return Ok(false);
    }
    let live = |field: u32| {
        field == WHOLE_VALUE
            || words
                .get(field as usize / 64)
                .is_none_or(|word| word >> (field % 64) & 1 == 1)
    };
    let mut at = 0;
    while at < d.steps.len() {
        let step = &d.steps[at];
        if live(step.field) {
            return Ok(false);
        }
        at += 1 + match step.kind {
            StepKind::Tag
            | StepKind::NonNull
            | StepKind::LiveBit
            | StepKind::Loop
            | StepKind::Column => step.span as usize,
            _ => 0,
        };
    }
    Ok(true)
}

/// Marks the release that ends `steps[start..end]`, one frame's steps, as `ReleaseLast`: its
/// last step, when that is a `Release`, or else the release ending each arm of the variant
/// whose arms are its last steps (`Tag` steps on one tag word, or one `NonNull` or `LiveBit`
/// arm). It is the drop program's last step through the arms of a last `ActiveVariant`, which
/// compiled drop glue frees by the same rule; a loop's steps are never marked.
fn mark_last(steps: &mut [Step], start: usize, end: usize) {
    let spanning = |kind: StepKind| {
        matches!(
            kind,
            StepKind::Tag
                | StepKind::NonNull
                | StepKind::LiveBit
                | StepKind::Loop
                | StepKind::Column
        )
    };
    let mut items = Vec::new();
    let mut at = start;
    while at < end {
        items.push(at);
        at += 1 + if spanning(steps[at].kind) {
            steps[at].span as usize
        } else {
            0
        };
    }
    let Some(&last) = items.last() else {
        return;
    };
    let arm = |steps: &mut [Step], at: usize| {
        let close = at + 1 + steps[at].span as usize;
        mark_last(steps, at + 1, close);
    };
    match steps[last].kind {
        StepKind::Release => steps[last].kind = StepKind::ReleaseLast,
        StepKind::Tag => {
            let offset = steps[last].offset;
            let arms: Vec<usize> = items
                .iter()
                .rev()
                .take_while(|&&i| steps[i].kind == StepKind::Tag && steps[i].offset == offset)
                .copied()
                .collect();
            for at in arms {
                arm(steps, at);
            }
        }
        StepKind::NonNull | StepKind::LiveBit => arm(steps, last),
        _ => {}
    }
}

/// Descriptors for `roots` and every layout their `Finalize` steps name,
/// transitively, each once, in first-reference order.
pub fn descriptors(
    table: &LayoutTable,
    roots: &[LayoutId],
) -> Result<Vec<Descriptor>, DescriptorError> {
    let storage = crate::logical_storage::MapStorage::new(table);
    descriptors_with_storage(table, roots, &storage)
}
pub fn descriptors_with_storage(
    table: &LayoutTable,
    roots: &[LayoutId],
    storage: &crate::logical_storage::MapStorage,
) -> Result<Vec<Descriptor>, DescriptorError> {
    let mut out: Vec<Descriptor> = Vec::new();
    let mut queue: Vec<LayoutId> = roots.to_vec();
    let mut next = 0;
    while next < queue.len() {
        let id = queue[next];
        next += 1;
        if out.iter().any(|d| d.layout == id) {
            continue;
        }
        let d = descriptor_with_storage(table, id, storage)?;
        queue.extend(d.referenced());
        out.push(d);
    }
    Ok(out)
}

fn lookup(table: &LayoutTable, id: LayoutId) -> Result<&LayoutRecord, DescriptorError> {
    table.layout(id).ok_or(DescriptorError::MissingLayout(id))
}

/// Every layout `scan` names, directly or nested, is in the table.
fn scanned(table: &LayoutTable, scan: &[ScanStep]) -> Result<(), DescriptorError> {
    for step in scan {
        match step {
            ScanStep::Inline { layout, .. }
            | ScanStep::Repeat {
                element: layout, ..
            } => {
                scanned(table, &lookup(table, *layout)?.scan)?;
            }
            ScanStep::ActiveVariant { variants, .. } => {
                for (_, steps) in variants {
                    scanned(table, steps)?;
                }
            }
            _ => {}
        }
    }
    Ok(())
}

fn field_count(fields: &[FieldLayout]) -> u32 {
    fields.iter().map(|f| f.ordinal + 1).max().unwrap_or(0)
}

/// The bytes every value of a dynamic layout has: its tag and fixed-size fields.
pub(crate) fn minimum_extent(
    table: &LayoutTable,
    record: &LayoutRecord,
) -> Result<u64, DescriptorError> {
    let mut extent = record
        .tag
        .as_ref()
        .filter(|t| t.form == TagForm::Ordinal)
        .map_or(0, |t| t.offset + 4);
    let fields = record.fields.iter().chain(
        record
            .tag
            .iter()
            .flat_map(|t| &t.variants)
            .flat_map(|v| &v.fields),
    );
    for field in fields {
        if let SizeSpec::Fixed(size) = lookup(table, field.layout)?.size {
            extent = extent.max(field.offset + size);
        }
    }
    Ok(extent)
}

struct Lower<'t> {
    table: &'t LayoutTable,
    steps: Vec<Step>,
    /// Loops enclosing the steps being lowered.
    loops: u32,
    scope: (LayoutId, u64),
    arms: Vec<(u64, u32)>,
}

impl Lower<'_> {
    /// Resolve a release through the active fields, preserving the distinction between
    /// a slice owner and a whole-block holder even when variant arms overlap.
    fn slice_owner(&self, offset: u64) -> bool {
        let (mut id, mut base) = self.scope;
        loop {
            let Some(record) = self.table.layout(id) else {
                return false;
            };
            if matches!(record.kind, LayoutKind::Slice { .. }) {
                return offset == base;
            }
            let fields = match &record.tag {
                Some(tag) => {
                    let Some((_, active)) = self
                        .arms
                        .iter()
                        .rev()
                        .find(|(at, _)| *at == base + tag.offset)
                    else {
                        return false;
                    };
                    let Some(variant) = tag.variants.iter().find(|v| v.ordinal == *active) else {
                        return false;
                    };
                    &variant.fields
                }
                None => &record.fields,
            };
            let Some(field) = fields.iter().find(|f| {
                matches!(self.table.layout(f.layout).map(|r| &r.size), Some(SizeSpec::Fixed(size)) if base + f.offset <= offset && offset - base - f.offset < *size)
            }) else { return false };
            id = field.layout;
            base += field.offset;
        }
    }

    fn push(&mut self, kind: StepKind, offset: u64) -> usize {
        self.steps.push(Step {
            kind,
            field: WHOLE_VALUE,
            variant: 0,
            span: 0,
            offset,
            stride: 0,
            count: 0,
            count_offset: NO_OFFSET,
            layout: None,
        });
        self.steps.len() - 1
    }

    /// Push a step that owns the steps lowered until `close`.
    fn open(&mut self, kind: StepKind, field: u32, set: impl FnOnce(&mut Step)) -> usize {
        let at = self.push(kind, 0);
        self.steps[at].field = field;
        set(&mut self.steps[at]);
        at
    }

    /// Open the step that keeps variant `ordinal`'s steps, its ordinal read
    /// as `form` at `offset`.
    fn arm(&mut self, form: TagForm, ordinal: u32, offset: u64) -> Result<usize, DescriptorError> {
        let kind = match form {
            TagForm::Ordinal => StepKind::Tag,
            TagForm::LiveBit { bit } if ordinal == 1 && bit < 64 => StepKind::LiveBit,
            TagForm::LiveBit { .. } => return malformed("drop steps for a cleared live bit"),
            TagForm::Niche { dataful, .. } if ordinal == dataful => StepKind::NonNull,
            TagForm::Niche { .. } => {
                return malformed("drop steps for the null variant of a niche");
            }
        };
        Ok(self.open(kind, WHOLE_VALUE, |s| {
            s.offset = offset;
            if let TagForm::LiveBit { bit } = form {
                s.variant = bit;
            }
            if kind == StepKind::Tag {
                s.variant = ordinal;
            }
        }))
    }

    fn close(&mut self, at: usize) -> Result<(), DescriptorError> {
        let span = self.steps.len() - at - 1;
        self.steps[at].span = u32::try_from(span)
            .map_err(|_| DescriptorError::TooLarge(format!("a span of {span} steps")))?;
        Ok(())
    }

    fn contribution(&self, field: &FieldLayout) -> Result<Vec<DropStep>, DescriptorError> {
        let child = lookup(self.table, field.layout)?;
        Ok(if child.finalizer.is_some() {
            vec![DropStep::Child {
                offset: field.offset,
                layout: field.layout,
            }]
        } else {
            child
                .drop
                .iter()
                .map(|step| shift_drop(step, field.offset))
                .collect()
        })
    }

    /// Whether `field`, at `range`, is held as a borrowed view: every scan
    /// step in its bytes is one of its borrowed scan's (`borrowed_scan`), as
    /// the layout validator's `borrowed_fields` reads it. A borrowed handle
    /// stays a `Handle` step, so only that comparison tells it from an owned
    /// one whose drop steps are missing.
    fn borrowed(
        &self,
        field: &FieldLayout,
        range: &std::ops::Range<u64>,
        scan: &[ScanStep],
    ) -> Result<bool, DescriptorError> {
        let child = lookup(self.table, field.layout)?;
        scanned(self.table, &child.scan)?;
        let record = |id| {
            self.table
                .layout(id)
                .expect("`scanned` found every layout the scan names")
        };
        let Some(view) = borrowed_scan(child, field.offset, &record) else {
            return Ok(false);
        };
        Ok(scan.iter().all(|step| {
            let at = match step {
                ScanStep::Counted { offset }
                | ScanStep::Root { offset }
                | ScanStep::Uncounted { offset }
                | ScanStep::SliceOwner { offset }
                | ScanStep::Environment { offset }
                | ScanStep::Handle { offset, .. }
                | ScanStep::Inline { offset, .. }
                | ScanStep::Sequence { offset, .. }
                | ScanStep::Repeat { offset, .. } => Some(*offset),
                ScanStep::ActiveVariant { tag_offset, .. } => Some(*tag_offset),
                ScanStep::Columns(_) => None,
            };
            at.is_some_and(|at| !range.contains(&at)) || view.contains(step)
        }))
    }

    /// Lower `drop` as the fields' contributions, each step naming its field.
    /// Each declaration contributes its exact ordered recipe. A missing recipe
    /// contributes none only when independent scan evidence proves a borrowed
    /// view; equal offsets and zero-sized fields retain their declaration order.
    fn fields(
        &mut self,
        fields: &[FieldLayout],
        drop: &[DropStep],
        scan: &[ScanStep],
        live: Option<(u64, &[u32])>,
    ) -> Result<(), DescriptorError> {
        let mut rest = drop;
        for field in fields.iter().rev() {
            let contribution = self.contribution(field)?;
            let mut n = contribution.len();
            let guarded = live.and_then(|(offset, movable)| {
                movable
                    .iter()
                    .position(|&i| i == field.ordinal)
                    .map(|bit| (offset, bit as u32))
            });
            if let Some((offset, bit)) = guarded
                && n > 0
            {
                if !matches!(rest.first(), Some(DropStep::ActiveVariant { tag_offset, form: TagForm::LiveBit { bit: actual }, variants }) if *tag_offset == offset && *actual == bit && variants.len() == 1 && variants[0].0 == 1)
                {
                    return malformed("movable capture without its live guard");
                }
                n = 1;
            }

            let end = match lookup(self.table, field.layout)?.size {
                SizeSpec::Fixed(size) => field.offset.saturating_add(size),
                SizeSpec::Dynamic(_) => u64::MAX,
            };
            let range = field.offset..end;
            if n > 0 && guarded.is_none() && !rest.starts_with(&contribution) {
                if self.borrowed(field, &range, scan)? {
                    n = 0;
                } else {
                    return malformed("a field's ordered drop contribution is missing");
                }
            }
            if n > rest.len() {
                return malformed("a drop program shorter than its fields' contributions");
            }
            let (mine, tail) = rest.split_at(n);
            rest = tail;
            let start = self.steps.len();
            self.program(mine, 0)?;
            for step in &mut self.steps[start..] {
                step.field = field.ordinal;
            }
        }
        if !rest.is_empty() {
            return malformed("a drop program longer than its fields' contributions");
        }
        Ok(())
    }

    /// Lower `drop`, whose offsets are relative to `at` in the current frame.
    fn program(&mut self, drop: &[DropStep], at: u64) -> Result<(), DescriptorError> {
        for step in drop {
            match step {
                DropStep::Finalize => {
                    return malformed("a Finalize step that is not first in its layout's program");
                }
                DropStep::Release { offset } | DropStep::Environment { offset } => {
                    let slice = self.slice_owner(at + offset);
                    let step = self.push(StepKind::Release, at + offset);
                    // Descriptor v15: release stride 24 marks a String/Bytes triple.
                    self.steps[step].stride = if slice { 24 } else { 0 };
                }
                DropStep::ReleaseSealed { .. } => {
                    return malformed("sealed owner runtime drop is not linked");
                }
                DropStep::ReleaseReceipt { offset } => {
                    self.push(StepKind::ReleaseReceipt, at + offset);
                }
                DropStep::ReleaseRoot { offset } => {
                    self.push(StepKind::ReleaseRoot, at + offset);
                }
                DropStep::CloseHandle { offset } => {
                    self.push(StepKind::CloseHandle, at + offset);
                }
                DropStep::Thaw { offset, layout } => {
                    let step = self.push(StepKind::Thaw, at + offset);
                    self.steps[step].layout = Some(*layout);
                }
                DropStep::Child { offset, layout } => self.child(*layout, at + offset)?,
                DropStep::ActiveVariant {
                    tag_offset,
                    form,
                    variants,
                } => {
                    for (ordinal, steps) in variants {
                        let open = self.arm(*form, *ordinal, at + tag_offset)?;
                        self.arms.push((at + tag_offset, *ordinal));
                        self.program(steps, at)?;
                        self.arms.pop();
                        self.close(open)?;
                    }
                }
                DropStep::ElementsReverse {
                    count_offset,
                    offset,
                    stride,
                    element,
                } => {
                    self.outermost("ElementsReverse")?;
                    let open = self.open(StepKind::Loop, WHOLE_VALUE, |s| {
                        s.offset = at + offset;
                        s.stride = *stride;
                        s.count_offset = at + count_offset;
                    });
                    self.element(*element, open)?;
                }
                DropStep::RepeatReverse {
                    offset,
                    count,
                    stride,
                    element,
                } => {
                    let open = self.open(StepKind::Loop, WHOLE_VALUE, |s| {
                        s.offset = at + offset;
                        s.stride = *stride;
                        s.count = *count;
                    });
                    self.element(*element, open)?;
                }
                DropStep::ColumnsReverse(columns) => {
                    self.outermost("ColumnsReverse")?;
                    let open = self.open(StepKind::Loop, WHOLE_VALUE, |s| {
                        s.offset = at;
                        s.count_offset = at + columns.count_offset;
                    });
                    self.loops += 1;
                    for column in columns.columns.iter().rev() {
                        let inner = self.open(StepKind::Column, WHOLE_VALUE, |s| {
                            s.offset = columns.data_offset;
                            s.stride = column.stride;
                            s.count = column.prefix;
                            s.count_offset = columns.capacity_offset;
                        });
                        self.child(column.layout, 0)?;
                        self.close(inner)?;
                    }
                    self.loops -= 1;
                    self.close(open)?;
                }
            }
        }
        Ok(())
    }

    fn outermost(&self, what: &str) -> Result<(), DescriptorError> {
        if self.loops > 0 {
            return malformed(format!("a dynamic {what} inside a loop"));
        }
        Ok(())
    }

    /// The body of the loop opened at `open`: one element's program at its base.
    fn element(&mut self, element: LayoutId, open: usize) -> Result<(), DescriptorError> {
        self.loops += 1;
        self.child(element, 0)?;
        self.loops -= 1;
        self.close(open)
    }

    /// An inline child: its own finalizer, then its fields.
    fn child(&mut self, id: LayoutId, at: u64) -> Result<(), DescriptorError> {
        let record = lookup(self.table, id)?;
        let mut drop = record.drop.as_slice();
        if let Some(DropStep::Finalize) = drop.first() {
            if record.finalizer.is_none() {
                return malformed("a Finalize step in a layout without a finalizer");
            }
            let step = self.push(StepKind::Finalize, at);
            self.steps[step].layout = Some(id);
            drop = &drop[1..];
        }
        let scope = self.scope;
        self.scope = (id, at);
        let result = self.program(drop, at);
        if result.is_ok() && record.finalizer.is_some() {
            let end = self.push(StepKind::EndFinalize, at);
            self.steps[end].layout = Some(id);
        }
        self.scope = scope;
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout_types::{
        AbiInputs, CopyRecipe, InitializedBytes, LayoutIdentity, LayoutKind, TargetSpec,
    };

    fn record(kind: LayoutKind, size: u64, drop: Vec<DropStep>) -> LayoutRecord {
        LayoutRecord {
            key: LayoutIdentity {
                owner_revision: None,
                instance: None,
                target: TargetSpec::x86_64_unknown_linux_gnu().digest(),
                layout_version: crate::LAYOUT_VERSION,
            },
            kind,
            size: SizeSpec::Fixed(size),
            align: 8,
            fields: Vec::new(),
            tag: None,
            initialized: InitializedBytes::Fields,
            scan: Vec::new(),
            drop,
            copy: CopyRecipe::Bits,
            finalizer: None,
            storage: Vec::new(),
            abi: AbiInputs {
                scalars: Vec::new(),
                unaligned: false,
            },
        }
    }

    fn table(records: Vec<LayoutRecord>) -> LayoutTable {
        let mut table = LayoutTable::new(&TargetSpec::x86_64_unknown_linux_gnu());
        table.layouts = records;
        table
    }

    #[test]
    fn an_environment_releases_like_a_counted_holder() {
        let t = table(vec![record(
            LayoutKind::Callable { environment: None },
            16,
            vec![DropStep::Environment { offset: 8 }],
        )]);
        let d = descriptor(&t, LayoutId(0)).unwrap();
        assert_eq!(d.steps.len(), 1);
        // Its only release ends the program, so it is the last (`mark_last`).
        assert_eq!(
            (d.steps[0].kind, d.steps[0].offset),
            (StepKind::ReleaseLast, 8)
        );
        assert_eq!(d.steps[0].field, WHOLE_VALUE);
    }

    /// Only a fixed-size layout that is not flat drops the arena extent: a flat record keeps
    /// it even at a fixed size, and a dynamic layout's capacity is per block.
    #[test]
    fn only_fixed_non_flat_layouts_have_no_extent() {
        let mut dynamic = record(LayoutKind::Aggregate, 0, Vec::new());
        dynamic.size = SizeSpec::Dynamic(Vec::new());
        let t = table(vec![
            record(LayoutKind::Aggregate, 16, Vec::new()),
            record(LayoutKind::Flat, 16, Vec::new()),
            dynamic,
        ]);
        let flags = |i| descriptor(&t, LayoutId(i)).map(|d| d.flags & LAYOUT_NO_EXTENT);
        assert_eq!(flags(0).unwrap(), LAYOUT_NO_EXTENT);
        assert_eq!(flags(1).unwrap(), 0);
        assert_eq!(flags(2).unwrap(), 0);
    }

    /// A list block is never bare in the system-allocator archive: compiled capacity
    /// queries read a sequence block's metadata, which a bare block has none of, so no
    /// sequence payload drops the extent, not even one recorded at a fixed size.
    #[test]
    fn a_sequence_payload_always_keeps_its_extent() {
        let element = LayoutId(0);
        let t = table(vec![
            record(LayoutKind::Aggregate, 8, Vec::new()),
            record(LayoutKind::Sequence { element }, 16, Vec::new()),
            record(LayoutKind::Sequential { element }, 16, Vec::new()),
        ]);
        let flags = |i| descriptor(&t, LayoutId(i)).map(|d| d.flags & LAYOUT_NO_EXTENT);
        assert_eq!(flags(0).unwrap(), LAYOUT_NO_EXTENT);
        assert_eq!(flags(1).unwrap(), 0);
        assert_eq!(flags(2).unwrap(), 0);
    }

    #[test]
    fn a_finalize_step_must_lead_a_finalized_layout() {
        let t = table(vec![record(
            LayoutKind::Unit,
            0,
            vec![DropStep::Release { offset: 0 }, DropStep::Finalize],
        )]);
        assert!(matches!(
            descriptor(&t, LayoutId(0)),
            Err(DescriptorError::Malformed(_))
        ));
    }

    #[test]
    fn a_dynamic_loop_inside_a_loop_is_rejected() {
        let element = record(
            LayoutKind::Unit,
            16,
            vec![DropStep::ElementsReverse {
                count_offset: 0,
                offset: 8,
                stride: 8,
                element: LayoutId(2),
            }],
        );
        let outer = record(
            LayoutKind::Unit,
            32,
            vec![DropStep::RepeatReverse {
                offset: 0,
                count: 2,
                stride: 16,
                element: LayoutId(0),
            }],
        );
        let leaf = record(LayoutKind::Unit, 8, vec![DropStep::Release { offset: 0 }]);
        let t = table(vec![element, outer, leaf]);
        assert!(descriptor(&t, LayoutId(0)).is_ok());
        assert!(matches!(
            descriptor(&t, LayoutId(1)),
            Err(DescriptorError::Malformed(m)) if m.contains("inside a loop")
        ));
    }

    #[test]
    fn a_record_drop_program_must_partition_into_its_fields() {
        // A counted leaf: it scans the reference it releases.
        let leaf = || {
            let mut leaf = record(LayoutKind::Unit, 8, vec![DropStep::Release { offset: 0 }]);
            leaf.scan = vec![ScanStep::Counted { offset: 0 }];
            leaf
        };
        let mut pair = record(
            LayoutKind::Aggregate,
            16,
            vec![DropStep::Release { offset: 8 }],
        );
        pair.fields = [0, 8]
            .into_iter()
            .enumerate()
            .map(|(ordinal, offset)| FieldLayout {
                ordinal: ordinal as u32,
                offset,
                layout: LayoutId(0),
                unaligned: false,
            })
            .collect();
        // Both fields hold a counted reference: neither may go undropped.
        pair.scan = vec![
            ScanStep::Counted { offset: 0 },
            ScanStep::Counted { offset: 8 },
        ];
        let t = table(vec![leaf(), pair.clone()]);
        assert!(matches!(
            descriptor(&t, LayoutId(1)),
            Err(DescriptorError::Malformed(_))
        ));
        // A field the layout scans as uncounted (a borrowed view) owns no step.
        let mut borrowed = pair.clone();
        borrowed.scan[0] = ScanStep::Uncounted { offset: 0 };
        let t = table(vec![leaf(), borrowed]);
        let d = descriptor(&t, LayoutId(1)).unwrap();
        assert_eq!(
            d.steps
                .iter()
                .map(|s| (s.offset, s.field))
                .collect::<Vec<_>>(),
            [(8, 1)]
        );
        pair.drop.push(DropStep::Release { offset: 0 });
        let t = table(vec![leaf(), pair]);
        let d = descriptor(&t, LayoutId(1)).unwrap();
        assert_eq!(
            d.steps
                .iter()
                .map(|s| (s.offset, s.field))
                .collect::<Vec<_>>(),
            [(8, 1), (0, 0)]
        );
        assert_eq!(d.field_count, 2);
        let mut reversed = t.clone();
        reversed.layouts[1].drop.reverse();
        assert!(matches!(descriptor(&reversed, LayoutId(1)),
            Err(DescriptorError::Malformed(message)) if message.contains("ordered drop contribution")));
    }
    #[test]
    fn zero_sized_siblings_keep_their_declared_drop_contributions() {
        use hirsel_syntax::{DefPath, ItemPath, PackageId};
        let mut child = record(LayoutKind::Resource, 0, vec![DropStep::Finalize]);
        child.finalizer = Some(FinalizerRef {
            definition: DefPath(ItemPath {
                package: PackageId {
                    name: "test".into(),
                    version: "1".into(),
                },
                modules: vec![],
                name: "Child.drop".into(),
            }),
            body: 0,
            unsafe_ffi_package: false,
        });
        let mut parent = record(
            LayoutKind::Aggregate,
            0,
            vec![
                DropStep::Child {
                    offset: 0,
                    layout: LayoutId(0),
                },
                DropStep::Child {
                    offset: 0,
                    layout: LayoutId(0),
                },
            ],
        );
        parent.fields = (0..2)
            .map(|ordinal| FieldLayout {
                ordinal,
                offset: 0,
                layout: LayoutId(0),
                unaligned: false,
            })
            .collect();
        let t = table(vec![child, parent]);
        let descriptor = descriptor(&t, LayoutId(1)).unwrap();
        assert_eq!(
            descriptor
                .steps
                .iter()
                .map(|step| (step.kind, step.offset, step.field, step.layout))
                .collect::<Vec<_>>(),
            [
                (StepKind::Finalize, 0, 1, Some(LayoutId(0))),
                (StepKind::EndFinalize, 0, 1, Some(LayoutId(0))),
                (StepKind::Finalize, 0, 0, Some(LayoutId(0))),
                (StepKind::EndFinalize, 0, 0, Some(LayoutId(0)))
            ]
        );
    }
}
