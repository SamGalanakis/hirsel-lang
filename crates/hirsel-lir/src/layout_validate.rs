//! Independent layout and ABI validation.
//!
//! The validator never calls the derivation. It checks each record locally
//! against its own fields and child records: target scalar facts, alignment
//! arithmetic, field containment and overlap, tag placement, scan coverage,
//! drop order, copy recipes, dynamic size programs and that every inline
//! recursive path passes through an explicit block. Children are validated
//! too, so the local checks compose over the whole table.
use crate::abi::{extension_of, physical};
use crate::abi_types::*;
use crate::layout::{
    FLAT_PREFIX, SEQUENCE_CAPACITY_OFFSET, align_up, borrowed_scan, evaluate_size,
    sequence_elements_offset, sequential_elements_offset, shift_drop, shift_scan, soa_data_offset,
};
use crate::layout_types::*;
use crate::runtime_abi::{HEADER_SIZE, HEADER_V2};

/// The obligation a violation breaks.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Ord, PartialOrd)]
pub enum Obligation {
    Identity,
    Reference,
    Alignment,
    Size,
    Offset,
    Overlap,
    Tag,
    Scan,
    DropOrder,
    Copy,
    SizeProgram,
    Recursion,
    Abi,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Subject {
    Table,
    Layout(LayoutId),
    FnAbi(FnAbiId),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Violation {
    pub subject: Subject,
    pub obligation: Obligation,
    pub detail: String,
}

/// Validate every layout record and FnAbi of `table` for `target`.
pub fn validate_table(table: &LayoutTable, target: &TargetSpec) -> Result<(), Vec<Violation>> {
    let mut v = Validator {
        table,
        target,
        out: Vec::new(),
    };
    v.table_identity();
    for (i, record) in table.layouts.iter().enumerate() {
        let id = LayoutId(i as u32);
        if v.references(id, record) {
            v.record(id, record);
        }
    }
    v.recursion();
    for (i, abi) in table.fn_abis.iter().enumerate() {
        v.fn_abi(FnAbiId(i as u32), abi);
    }
    if v.out.is_empty() { Ok(()) } else { Err(v.out) }
}

struct Validator<'a> {
    table: &'a LayoutTable,
    target: &'a TargetSpec,
    out: Vec<Violation>,
}

/// Ordinary storage reserves a bounded Packed value's largest extent. Flat
/// fields retain their format's prefix instead of ordinary storage padding.
fn static_size(record: &LayoutRecord, in_flat: bool) -> Option<u64> {
    match &record.size {
        SizeSpec::Fixed(n) => Some(*n),
        SizeSpec::Dynamic(_) if in_flat => Some(FLAT_PREFIX),
        SizeSpec::Dynamic(_) if matches!(record.kind, LayoutKind::Packed) => record.size.storage(),
        SizeSpec::Dynamic(_) => None,
    }
}

impl<'a> Validator<'a> {
    fn fail(&mut self, subject: Subject, obligation: Obligation, detail: impl Into<String>) {
        self.out.push(Violation {
            subject,
            obligation,
            detail: detail.into(),
        });
    }
    fn get(&self, id: LayoutId) -> &'a LayoutRecord {
        &self.table.layouts[id.0 as usize]
    }
    fn in_range(&self, id: LayoutId) -> bool {
        (id.0 as usize) < self.table.layouts.len()
    }

    fn table_identity(&mut self) {
        if self.table.target != self.target.digest() {
            self.fail(
                Subject::Table,
                Obligation::Identity,
                "table target digest differs from the target",
            );
        }
        if self.table.layout_version != crate::LAYOUT_VERSION
            || self.table.abi_version != crate::ABI_VERSION
        {
            self.fail(
                Subject::Table,
                Obligation::Identity,
                "stale layout or ABI version",
            );
        }
    }

    /// Every LayoutId a record names is in the table; later checks index freely.
    fn references(&mut self, id: LayoutId, r: &LayoutRecord) -> bool {
        let mut ids = Vec::new();
        match &r.kind {
            LayoutKind::Inline { element, .. }
            | LayoutKind::Simd { element, .. }
            | LayoutKind::Slice { element }
            | LayoutKind::Iterator { element }
            | LayoutKind::SoaColumns { element }
            | LayoutKind::Sequence { element }
            | LayoutKind::Sequential { element } => ids.push(*element),
            LayoutKind::Block { payload }
            | LayoutKind::Box { payload }
            | LayoutKind::Shared { payload } => ids.push(*payload),
            LayoutKind::SealedRegion { root } => ids.push(*root),
            LayoutKind::Frozen { value } => ids.push(*value),
            LayoutKind::Callable { environment } => ids.extend(*environment),
            _ => {}
        }
        ids.extend(r.fields.iter().map(|f| f.layout));
        if let Some(tag) = &r.tag {
            ids.extend(
                tag.variants
                    .iter()
                    .flat_map(|v| v.fields.iter().map(|f| f.layout)),
            );
        }
        r.scan.iter().for_each(|s| scan_ids(s, &mut ids));
        r.drop.iter().for_each(|d| drop_ids(d, &mut ids));
        let ok = ids.iter().all(|&i| self.in_range(i));
        if !ok {
            self.fail(
                Subject::Layout(id),
                Obligation::Reference,
                "names a layout outside the table",
            );
        }
        ok
    }

    fn record(&mut self, id: LayoutId, r: &LayoutRecord) {
        let s = Subject::Layout(id);
        if r.key.target != self.table.target || r.key.layout_version != self.table.layout_version {
            self.fail(
                s.clone(),
                Obligation::Identity,
                "key names another target or layout version",
            );
        }
        if r.align == 0 || !r.align.is_power_of_two() {
            self.fail(
                s.clone(),
                Obligation::Alignment,
                format!("alignment {} is not a power of two", r.align),
            );
            return;
        }
        if let SizeSpec::Fixed(n) = r.size
            && n % u64::from(r.align) != 0
        {
            self.fail(
                s.clone(),
                Obligation::Size,
                format!("size {n} is not a multiple of alignment {}", r.align),
            );
        }
        if let SizeSpec::Dynamic(program) = &r.size {
            self.program(&s, program);
        }
        let pointer = self.target.scalar_size(ScalarKind::Pointer);
        let pointer_align = self.target.scalar_align(ScalarKind::Pointer);
        let in_flat = matches!(r.kind, LayoutKind::Flat)
            || (matches!(r.kind, LayoutKind::Packed) && r.align == 1);
        match &r.kind {
            LayoutKind::Unit => {
                self.exact(&s, r, 0, 1);
                self.no_steps(&s, r);
            }
            LayoutKind::Scalar(k) => {
                self.exact(
                    &s,
                    r,
                    self.target.scalar_size(*k),
                    self.target.scalar_align(*k),
                );
                self.no_steps(&s, r);
            }
            LayoutKind::Header => self.header(&s, r),
            LayoutKind::Block { .. } | LayoutKind::Box { .. } => {
                self.exact(&s, r, pointer, pointer_align);
                self.wrapper(
                    &s,
                    r,
                    &[ScanStep::Counted { offset: 0 }],
                    &[DropStep::Release { offset: 0 }],
                );
                if r.copy != CopyRecipe::Dup && r.copy != CopyRecipe::FlatBytes {
                    self.fail(
                        s.clone(),
                        Obligation::Copy,
                        "a block reference copies by adding a credit",
                    );
                }
            }
            LayoutKind::PayloadReceipt { .. } => {
                if !r.fields.is_empty() || r.tag.is_some() || r.finalizer.is_some() {
                    self.fail(
                        s.clone(),
                        Obligation::DropOrder,
                        "receipt metadata has no language fields or finalizer",
                    );
                }
                self.exact(&s, r, pointer, pointer_align);
                self.wrapper(&s, r, &[], &[DropStep::ReleaseReceipt { offset: 0 }]);
                self.copy(&s, r, CopyRecipe::NotCopyable);
            }
            LayoutKind::Shared { payload } => {
                self.exact(&s, r, pointer, pointer_align);
                self.wrapper(
                    &s,
                    r,
                    &[ScanStep::Root { offset: 0 }],
                    &[DropStep::ReleaseRoot { offset: 0 }],
                );
                self.copy(&s, r, CopyRecipe::Dup);
                if !matches!(self.get(*payload).kind, LayoutKind::Frozen { .. }) {
                    self.fail(
                        s.clone(),
                        Obligation::Reference,
                        "a Shared root's payload is a Frozen value",
                    );
                }
            }
            LayoutKind::SealedRegion { .. } => {
                self.exact(&s, r, pointer, pointer_align);
                self.wrapper(
                    &s,
                    r,
                    &[ScanStep::Uncounted { offset: 0 }],
                    &[DropStep::ReleaseSealed { offset: 0 }],
                );
                self.copy(&s, r, CopyRecipe::NotCopyable);
            }
            LayoutKind::Frozen { value } => self.frozen(&s, r, *value),
            LayoutKind::Slice { .. } => {
                self.exact(&s, r, pointer + 16, pointer_align.max(8));
                self.wrapper(
                    &s,
                    r,
                    &[ScanStep::SliceOwner { offset: 0 }],
                    &[DropStep::Release { offset: 0 }],
                );
                self.copy(&s, r, CopyRecipe::Dup);
            }
            // A static callable (an `Entry`, RUN-3) is its code and the static empty
            // environment, which is never counted: plain bits.
            LayoutKind::Callable { environment: None }
                if r.scan.is_empty() && r.drop.is_empty() && r.copy == CopyRecipe::Bits =>
            {
                self.exact(&s, r, 2 * pointer, pointer_align);
            }
            LayoutKind::Callable { .. } => {
                self.exact(&s, r, 2 * pointer, pointer_align);
                self.wrapper(
                    &s,
                    r,
                    &[ScanStep::Environment { offset: pointer }],
                    &[DropStep::Environment { offset: pointer }],
                );
                if !matches!(r.copy, CopyRecipe::Dup | CopyRecipe::NotCopyable) {
                    self.fail(
                        s.clone(),
                        Obligation::Copy,
                        "a callable copies its environment credit or not at all",
                    );
                }
            }
            LayoutKind::Iterator { .. } => {
                self.exact(&s, r, 3 * pointer, pointer_align);
                self.wrapper(
                    &s,
                    r,
                    &[ScanStep::Environment { offset: 0 }],
                    &[DropStep::Environment { offset: 0 }],
                );
                self.copy(&s, r, CopyRecipe::NotCopyable);
            }
            LayoutKind::Path => {
                self.exact(&s, r, 8, self.target.scalar_align(ScalarKind::U64));
                if !r.scan.is_empty() || !r.drop.is_empty() || r.finalizer.is_some() {
                    self.fail(s.clone(), Obligation::Scan, "Path owns no content");
                }
                self.copy(&s, r, CopyRecipe::Bits);
            }
            LayoutKind::Arena => {
                self.exact(&s, r, pointer, pointer_align);
                if !r.scan.is_empty() || !r.drop.is_empty() || r.finalizer.is_some() {
                    self.fail(s.clone(), Obligation::Scan, "an arena token owns nothing");
                }
                self.copy(&s, r, CopyRecipe::Bits);
            }
            LayoutKind::Handle | LayoutKind::HostCapability { .. } => {
                if let LayoutKind::HostCapability { kind } = &r.kind
                    && !hirsel_core_contract::handle_scan::host_capability_kind(kind)
                {
                    self.fail(
                        s.clone(),
                        Obligation::Scan,
                        "unknown host capability observation kind",
                    );
                }
                self.exact(&s, r, 8, self.target.scalar_align(ScalarKind::U64));
                if !matches!(r.scan.as_slice(), [ScanStep::Handle { offset: 0, .. }]) {
                    self.fail(s.clone(), Obligation::Scan, "a handle scans its ID");
                }
                if let [
                    ScanStep::Handle {
                        kind, ty, obtains, ..
                    },
                ] = r.scan.as_slice()
                {
                    let typed = *kind != HandleKind::Unknown;
                    let has_payload = matches!(
                        kind,
                        HandleKind::Receiver
                            | HandleKind::Mutex
                            | HandleKind::Task
                            | HandleKind::TaskSet
                    );
                    if typed != ty.is_some() || has_payload != obtains.is_some() {
                        self.fail(
                            s.clone(),
                            Obligation::Scan,
                            "typed handle scans name their handle and obtainable payload layouts",
                        );
                    }
                }
                let closes = r.drop == [DropStep::CloseHandle { offset: 0 }];
                if !(closes || r.drop.is_empty()) {
                    self.fail(
                        s.clone(),
                        Obligation::DropOrder,
                        "a handle drop closes its ID or does nothing",
                    );
                }
                self.copy(
                    &s,
                    r,
                    if closes {
                        CopyRecipe::NotCopyable
                    } else {
                        CopyRecipe::Bits
                    },
                );
            }
            LayoutKind::Inline { element, count } => self.inline(&s, r, *element, *count),
            LayoutKind::Simd { element, lanes } => {
                let lane = self.get(*element);
                let lane_size = match lane.kind {
                    LayoutKind::Scalar(k) if k != ScalarKind::Pointer => self.target.scalar_size(k),
                    _ => {
                        self.fail(
                            s.clone(),
                            Obligation::Size,
                            "Simd lanes are bools, sized integers or floats",
                        );
                        return;
                    }
                };
                match u64::from(*lanes).checked_mul(lane_size) {
                    Some(bytes)
                        if *lanes > 0
                            && r.size
                                == SizeSpec::Fixed(
                                    align_up(bytes, u64::from(r.align)).unwrap_or(0),
                                ) => {}
                    _ => self.fail(
                        s.clone(),
                        Obligation::Size,
                        "Simd size is the lanes rounded to its alignment",
                    ),
                }
                self.no_steps(&s, r);
            }
            LayoutKind::Sequence { element } => self.sequence(&s, r, *element),
            LayoutKind::Sequential { element } => self.sequential(&s, r, *element),
            LayoutKind::SoaColumns { element } => self.soa(&s, r, *element),
            LayoutKind::Environment { movable } => self.environment(&s, id, r, movable),
            LayoutKind::Aggregate
            | LayoutKind::Resource
            | LayoutKind::Opaque { .. }
            | LayoutKind::Flat
                if r.tag.is_none() =>
            {
                let end = self.fields(&s, &r.fields, r, in_flat);
                if let (Some(end), SizeSpec::Fixed(size)) = (end, &r.size)
                    && align_up(end, u64::from(r.align)).ok() != Some(*size)
                    && !matches!(r.kind, LayoutKind::Flat)
                {
                    self.fail(
                        s.clone(),
                        Obligation::Size,
                        format!(
                            "size {size} is not the field end {end} rounded to {}",
                            r.align
                        ),
                    );
                }
                if matches!(r.kind, LayoutKind::Flat) {
                    self.flat_extent(&s, r, end);
                }
                self.composed(&s, r, &r.fields);
            }
            LayoutKind::Aggregate
            | LayoutKind::Resource
            | LayoutKind::Opaque { .. }
            | LayoutKind::Variant
            | LayoutKind::Packed => self.variants(&s, r, in_flat),
            LayoutKind::Flat => self.fail(s, Obligation::Tag, "a flat record has no tag"),
        }
    }

    fn environment(&mut self, s: &Subject, id: LayoutId, r: &LayoutRecord, movable: &[u32]) {
        let Some(word) = r.fields.last() else {
            self.fail(s.clone(), Obligation::Offset, "environment lacks live word");
            return;
        };
        if movable.is_empty()
            || movable.len() > 64
            || movable.windows(2).any(|w| w[0] >= w[1])
            || word.ordinal as usize + 1 != r.fields.len()
            || movable
                .iter()
                .any(|&i| i as usize >= r.fields.len().saturating_sub(1))
            || r.tag.is_some()
            || self.get(word.layout).kind != LayoutKind::Scalar(ScalarKind::U64)
        {
            self.fail(
                s.clone(),
                Obligation::Offset,
                "invalid environment live word",
            );
            return;
        }
        let mut plain = r.clone();
        plain.kind = LayoutKind::Aggregate;
        plain.scan.clear();
        plain.drop.clear();
        let mut guarded_scan = std::collections::BTreeSet::new();
        let mut guarded_drop = std::collections::BTreeSet::new();
        for step in &r.scan {
            if let ScanStep::ActiveVariant {
                tag_offset,
                form: TagForm::LiveBit { bit },
                variants,
            } = step
            {
                let field = movable
                    .get(*bit as usize)
                    .and_then(|&i| r.fields.get(i as usize));
                let expected = field.map(|f| {
                    self.get(f.layout)
                        .scan
                        .iter()
                        .map(|s| shift_scan(s, f.offset))
                        .collect::<Vec<_>>()
                });
                if *tag_offset != word.offset
                    || !guarded_scan.insert(*bit)
                    || expected
                        .as_ref()
                        .is_none_or(|e| variants != &vec![(1, e.clone())])
                {
                    self.fail(s.clone(), Obligation::Scan, "invalid capture scan guard");
                }
                if let Some((_, steps)) = variants.first() {
                    plain.scan.extend(steps.iter().cloned());
                }
            } else {
                plain.scan.push(step.clone());
            }
        }
        for step in &r.drop {
            if let DropStep::ActiveVariant {
                tag_offset,
                form: TagForm::LiveBit { bit },
                variants,
            } = step
            {
                let field = movable
                    .get(*bit as usize)
                    .and_then(|&i| r.fields.get(i as usize));
                let expected = field.map(|f| {
                    let child = self.get(f.layout);
                    if child.finalizer.is_some() {
                        vec![DropStep::Child {
                            offset: f.offset,
                            layout: f.layout,
                        }]
                    } else {
                        child.drop.iter().map(|s| shift_drop(s, f.offset)).collect()
                    }
                });
                if *tag_offset != word.offset
                    || !guarded_drop.insert(*bit)
                    || expected
                        .as_ref()
                        .is_none_or(|e| variants != &vec![(1, e.clone())])
                {
                    self.fail(
                        s.clone(),
                        Obligation::DropOrder,
                        "invalid capture drop guard",
                    );
                }
                if let Some((_, steps)) = variants.first() {
                    plain.drop.extend(steps.iter().cloned());
                }
            } else {
                plain.drop.push(step.clone());
            }
        }
        for (bit, &ordinal) in movable.iter().enumerate() {
            let child = self.get(r.fields[ordinal as usize].layout);
            if (!child.scan.is_empty() && !guarded_scan.contains(&(bit as u32)))
                || (!child.drop.is_empty() && !guarded_drop.contains(&(bit as u32)))
            {
                self.fail(
                    s.clone(),
                    Obligation::DropOrder,
                    "movable capture lacks live guard",
                );
            }
        }
        plain.scan.sort_by_key(crate::layout::scan_order);
        self.record(id, &plain);
    }

    fn exact(&mut self, s: &Subject, r: &LayoutRecord, size: u64, align: u32) {
        if r.size != SizeSpec::Fixed(size) {
            self.fail(s.clone(), Obligation::Size, format!("expected size {size}"));
        }
        if r.align != align {
            self.fail(
                s.clone(),
                Obligation::Alignment,
                format!("expected alignment {align}"),
            );
        }
    }

    fn no_steps(&mut self, s: &Subject, r: &LayoutRecord) {
        if !r.scan.is_empty() {
            self.fail(
                s.clone(),
                Obligation::Scan,
                "a holder-free layout has scan steps",
            );
        }
        if !r.drop.is_empty() {
            self.fail(
                s.clone(),
                Obligation::DropOrder,
                "a holder-free layout has drop steps",
            );
        }
    }

    fn copy(&mut self, s: &Subject, r: &LayoutRecord, expected: CopyRecipe) {
        if r.copy != expected {
            self.fail(
                s.clone(),
                Obligation::Copy,
                format!("expected copy recipe {expected:?}"),
            );
        }
    }

    fn wrapper(&mut self, s: &Subject, r: &LayoutRecord, scan: &[ScanStep], drop: &[DropStep]) {
        if r.scan != scan {
            self.fail(
                s.clone(),
                Obligation::Scan,
                "a wrapper scans exactly its holder word",
            );
        }
        if r.drop != drop {
            self.fail(
                s.clone(),
                Obligation::DropOrder,
                "a wrapper drops exactly its holder word",
            );
        }
    }

    fn header(&mut self, s: &Subject, r: &LayoutRecord) {
        self.exact(s, r, HEADER_SIZE, 8);
        let expected: Vec<_> = HEADER_V2.iter().map(|&(_, k, o)| (o, k)).collect();
        let actual: Vec<_> = r
            .fields
            .iter()
            .map(|f| match self.get(f.layout).kind {
                LayoutKind::Scalar(k) => (f.offset, k),
                _ => (f.offset, ScalarKind::Bool),
            })
            .collect();
        if actual != expected {
            self.fail(
                s.clone(),
                Obligation::Offset,
                "header fields differ from hirsel_rt_v2_header",
            );
        }
        self.no_steps(s, r);
    }

    /// Field ordinals, alignment, containment and overlap; returns the end.
    fn fields(
        &mut self,
        s: &Subject,
        fields: &[FieldLayout],
        r: &LayoutRecord,
        in_flat: bool,
    ) -> Option<u64> {
        let mut spans = Vec::new();
        let mut natural = 1u32;
        for (i, field) in fields.iter().enumerate() {
            if field.ordinal != i as u32 {
                self.fail(
                    s.clone(),
                    Obligation::Offset,
                    "field ordinals are not in declaration order",
                );
            }
            let child = self.get(field.layout);
            let Some(size) = static_size(child, in_flat) else {
                self.fail(
                    s.clone(),
                    Obligation::Size,
                    format!("field {i} has a dynamic extent inline"),
                );
                return None;
            };
            if !field.unaligned {
                if field.offset % u64::from(child.align) != 0 {
                    self.fail(
                        s.clone(),
                        Obligation::Offset,
                        format!(
                            "field {i} at {} is misaligned for {}",
                            field.offset, child.align
                        ),
                    );
                }
                natural = natural.max(child.align);
            }
            let Some(end) = field.offset.checked_add(size) else {
                self.fail(s.clone(), Obligation::Size, "field extent overflows");
                return None;
            };
            spans.push((field.offset, end, i));
        }
        if r.align < natural {
            self.fail(
                s.clone(),
                Obligation::Alignment,
                format!("alignment {} is below the fields' {natural}", r.align),
            );
        }
        if in_flat {
            // FlatFormatV0 is declaration-order concatenation with no padding.
            let mut cursor = spans.first().map_or(0, |&(o, _, _)| o);
            for &(offset, end, i) in &spans {
                if offset != cursor {
                    self.fail(
                        s.clone(),
                        Obligation::Offset,
                        format!("flat field {i} is not at the cursor {cursor}"),
                    );
                }
                cursor = end;
            }
        }
        self.overlap(s, spans)
    }

    fn overlap(&mut self, s: &Subject, mut spans: Vec<(u64, u64, usize)>) -> Option<u64> {
        spans.sort();
        let mut end = 0;
        for w in spans.windows(2) {
            let ((_, a_end, a), (b_start, b_end, b)) = (w[0], w[1]);
            if a_end > b_start && b_end > b_start {
                self.fail(
                    s.clone(),
                    Obligation::Overlap,
                    format!("fields {a} and {b} overlap"),
                );
            }
        }
        for &(_, e, _) in &spans {
            end = end.max(e);
        }
        Some(end)
    }

    /// Scan and drop programs and the copy recipe must be the composition of
    /// the fields': scans in declaration order, drops in reverse declaration
    /// order after the record's own finalizer.
    fn composed(&mut self, s: &Subject, r: &LayoutRecord, fields: &[FieldLayout]) {
        let (scan, drop, copy) = self.compose(fields, &self.borrowed_fields(r, fields));
        let own_scan: Vec<_> = r
            .scan
            .iter()
            .filter(|step| !self.borrowed_slot(step, fields))
            .cloned()
            .collect();
        if own_scan != scan {
            self.fail(
                s.clone(),
                Obligation::Scan,
                "scan program does not cover exactly the fields' holders",
            );
        }
        let mut expected_drop = Vec::new();
        if r.finalizer.is_some() {
            expected_drop.push(DropStep::Finalize);
        }
        expected_drop.extend(drop);
        if r.drop != expected_drop {
            self.fail(
                s.clone(),
                Obligation::DropOrder,
                "drop program is not the finalizer then the fields high to low",
            );
        }
        let expected_copy = if r.kind == LayoutKind::Resource {
            CopyRecipe::NotCopyable
        } else if r.kind == LayoutKind::Flat {
            CopyRecipe::FlatBytes
        } else {
            copy
        };
        // Borrowed fields can make a plain physical record noncopyable: the
        // cursor's loan is carried by its source type, not by holder credits.
        // Preserve that restriction; a record of copyable plain fields still
        // has to advertise Bits, and an owned holder still needs its recipe.
        let borrowed_noncopyable = r.copy == CopyRecipe::NotCopyable
            && expected_copy == CopyRecipe::Bits
            && self.borrowed_fields(r, fields).into_iter().any(|b| b);
        if r.copy != expected_copy && !borrowed_noncopyable {
            self.fail(
                s.clone(),
                Obligation::Copy,
                format!("copy recipe {:?} is not {expected_copy:?}", r.copy),
            );
        }
        if r.finalizer.is_some() && r.kind != LayoutKind::Resource {
            self.fail(
                s.clone(),
                Obligation::DropOrder,
                "only a resource has a finalizer",
            );
        }
    }

    /// An uncounted environment slot: an address-sized field read as a borrow.
    fn borrowed_slot(&self, step: &ScanStep, fields: &[FieldLayout]) -> bool {
        let ScanStep::Uncounted { offset } = step else {
            return false;
        };
        fields.iter().any(|f| {
            f.offset == *offset
                && self.get(f.layout).kind == LayoutKind::Scalar(ScalarKind::Pointer)
        })
    }

    /// Fields held as borrowed views (a closure environment's borrowed
    /// captures): their holders appear in the record's scan as uncounted
    /// references instead of their own steps, and they are never dropped. A
    /// field reads as borrowed only when both programs say so.
    fn borrowed_fields(&self, r: &LayoutRecord, fields: &[FieldLayout]) -> Vec<bool> {
        fields
            .iter()
            .map(|f| crate::layout::field_is_borrowed(self.table, r, f))
            .collect()
    }

    fn compose(
        &self,
        fields: &[FieldLayout],
        borrowed: &[bool],
    ) -> (Vec<ScanStep>, Vec<DropStep>, CopyRecipe) {
        let mut scan = Vec::new();
        let mut drop = Vec::new();
        let mut copy = CopyRecipe::Bits;
        for (i, f) in fields.iter().enumerate() {
            let child = self.get(f.layout);
            if borrowed[i] {
                scan.extend(borrowed_scan(child, f.offset, &|id| self.get(id)).unwrap_or_default());
                continue;
            }
            scan.extend(child.scan.iter().map(|x| shift_scan(x, f.offset)));
            copy = match (copy, child.copy) {
                (CopyRecipe::NotCopyable, _) | (_, CopyRecipe::NotCopyable) => {
                    CopyRecipe::NotCopyable
                }
                (CopyRecipe::Bits, CopyRecipe::Bits) => CopyRecipe::Bits,
                _ => CopyRecipe::Fields,
            };
        }
        for (i, f) in fields.iter().enumerate().rev() {
            if borrowed[i] {
                continue;
            }
            let child = self.get(f.layout);
            if child.finalizer.is_some() {
                drop.push(DropStep::Child {
                    offset: f.offset,
                    layout: f.layout,
                });
            } else {
                drop.extend(child.drop.iter().map(|x| shift_drop(x, f.offset)));
            }
        }
        (scan, drop, copy)
    }

    fn variants(&mut self, s: &Subject, r: &LayoutRecord, in_flat: bool) {
        let Some(tag) = &r.tag else {
            self.fail(s.clone(), Obligation::Tag, "a variant layout has no tag");
            return;
        };
        if !r.fields.is_empty() {
            self.fail(
                s.clone(),
                Obligation::Tag,
                "variant payload fields belong to their variant",
            );
        }
        // A niche has no tag bytes of its own: the word is a field's.
        let tag_end = match tag.form {
            TagForm::LiveBit { .. } => {
                self.fail(s.clone(), Obligation::Tag, "live bits are not variant tags");
                return;
            }
            TagForm::Ordinal => {
                // Only `@packed` variants and flat Packed bodies have alignment 1.
                if r.align != 1 && tag.offset % 4 != 0 {
                    self.fail(s.clone(), Obligation::Tag, "the u32 tag is misaligned");
                }
                tag.offset + 4
            }
            TagForm::Niche { nullary, dataful } => {
                self.niche(s, r, tag, nullary, dataful);
                0
            }
        };
        let mut ends = Vec::new();
        let mut expected_scan = Vec::new();
        let mut expected_drop = Vec::new();
        let mut copy = CopyRecipe::Bits;
        for (i, variant) in tag.variants.iter().enumerate() {
            if variant.ordinal != i as u32 {
                self.fail(
                    s.clone(),
                    Obligation::Tag,
                    "variant ordinals are not 0..n in order",
                );
            }
            let mut spans = Vec::new();
            if tag.form == TagForm::Ordinal {
                spans.push((tag.offset, tag_end, usize::MAX));
            }
            let end = self.fields(s, &variant.fields, r, in_flat).unwrap_or(0);
            for (j, f) in variant.fields.iter().enumerate() {
                let size = static_size(self.get(f.layout), in_flat).unwrap_or(0);
                spans.push((f.offset, f.offset + size, j));
            }
            if self.overlap(s, spans).is_none() {
                return;
            }
            if tag.form == TagForm::Ordinal && variant.fields.iter().any(|f| f.offset < tag_end) {
                self.fail(
                    s.clone(),
                    Obligation::Tag,
                    format!("variant {i} overlaps the tag"),
                );
            }
            ends.push((variant.ordinal, end.max(tag_end)));
            let (scan, drop, c) = self.compose(&variant.fields, &vec![false; variant.fields.len()]);
            if !scan.is_empty() {
                expected_scan.push((variant.ordinal, scan));
            }
            if !drop.is_empty() {
                expected_drop.push((variant.ordinal, drop));
            }
            copy = match (copy, c) {
                (CopyRecipe::NotCopyable, _) | (_, CopyRecipe::NotCopyable) => {
                    CopyRecipe::NotCopyable
                }
                (CopyRecipe::Bits, CopyRecipe::Bits) => CopyRecipe::Bits,
                _ => CopyRecipe::Fields,
            };
        }
        let scan = if expected_scan.is_empty() {
            Vec::new()
        } else {
            vec![ScanStep::ActiveVariant {
                tag_offset: tag.offset,
                form: tag.form,
                variants: expected_scan,
            }]
        };
        if r.scan != scan {
            self.fail(
                s.clone(),
                Obligation::Scan,
                "scan program does not scan exactly the active variant's holders",
            );
        }
        let mut drop = Vec::new();
        if r.finalizer.is_some() {
            drop.push(DropStep::Finalize);
        }
        if !expected_drop.is_empty() {
            drop.push(DropStep::ActiveVariant {
                tag_offset: tag.offset,
                form: tag.form,
                variants: expected_drop,
            });
        }
        if r.drop != drop {
            self.fail(
                s.clone(),
                Obligation::DropOrder,
                "drop program is not the active variant's fields high to low",
            );
        }
        let expected_copy = match (&r.kind, in_flat) {
            (LayoutKind::Resource, _) => CopyRecipe::NotCopyable,
            (_, true) => CopyRecipe::FlatBytes,
            _ => copy,
        };
        if r.copy != expected_copy {
            self.fail(
                s.clone(),
                Obligation::Copy,
                format!("copy recipe {:?} is not {expected_copy:?}", r.copy),
            );
        }
        // Size: the largest variant rounded up for a variant; each active
        // variant's own extent for Packed.
        let align = u64::from(r.align);
        let largest = ends.iter().map(|&(_, e)| e).max().unwrap_or(tag_end);
        match (&r.kind, &r.size) {
            (LayoutKind::Packed, size) => {
                // With every length prefix zero, each active variant's extent
                // is its static payload end (rounded for in-memory Packed).
                for &(ordinal, end) in &ends {
                    let mut bytes = vec![0u8; usize::try_from(end).unwrap_or(0).saturating_add(64)];
                    let at = tag.offset as usize;
                    bytes[at..at + 4].copy_from_slice(&ordinal.to_le_bytes());
                    let expected = if in_flat {
                        Some(end)
                    } else {
                        align_up(end, align).ok()
                    };
                    if evaluate_size(size, &bytes).ok() != expected {
                        self.fail(
                            s.clone(),
                            Obligation::Size,
                            format!("Packed variant {ordinal} extent is not its payload end"),
                        );
                    }
                }
            }
            (_, SizeSpec::Fixed(size)) => {
                if align_up(largest, align).ok() != Some(*size) {
                    self.fail(s.clone(), Obligation::Size, format!("size {size} is not the largest variant end {largest} rounded to {align}"));
                }
            }
            (_, SizeSpec::Dynamic(_)) => {
                self.fail(
                    s.clone(),
                    Obligation::Size,
                    "an ordinary variant has a fixed size",
                );
            }
        }
    }

    /// A null-pointer niche (layout version 5): an ordinary in-memory variant
    /// of two variants, `nullary` without fields and `dataful` whose payload
    /// holds an aligned non-null pointer word at the tag offset (`nonnull_word`).
    fn niche(
        &mut self,
        s: &Subject,
        r: &LayoutRecord,
        tag: &TagEncoding,
        nullary: u32,
        dataful: u32,
    ) {
        if r.kind != LayoutKind::Variant || r.initialized != InitializedBytes::Fields {
            self.fail(
                s.clone(),
                Obligation::Tag,
                "a niche tag outside an ordinary in-memory variant",
            );
        }
        let ordinals: Vec<u32> = tag.variants.iter().map(|v| v.ordinal).collect();
        if ordinals.len() != 2
            || nullary == dataful
            || !ordinals.contains(&nullary)
            || !ordinals.contains(&dataful)
        {
            self.fail(
                s.clone(),
                Obligation::Tag,
                "a niche names other than the variant's two ordinals",
            );
            return;
        }
        let fields = |ordinal: u32| {
            tag.variants
                .iter()
                .find(|v| v.ordinal == ordinal)
                .map_or(&[][..], |v| &v.fields[..])
        };
        if !fields(nullary).is_empty() {
            self.fail(
                s.clone(),
                Obligation::Tag,
                "the niche's null variant has fields",
            );
        }
        let align = u64::from(self.target.scalar_align(ScalarKind::Pointer));
        if tag.offset % align != 0
            || !self.nonnull_word(fields(dataful), tag.offset, self.table.layouts.len())
        {
            self.fail(
                s.clone(),
                Obligation::Tag,
                "the niche word is not an aligned non-null pointer of the other variant",
            );
        }
    }

    /// Whether the pointer word at `offset` among `fields` is never null while
    /// they are live: it is a counted block reference, Ptr or FnPtr leaf, or a
    /// String or Bytes slice's owner word, reached only through aligned fields
    /// of inline Aggregates. A word inside a variant, environment, callable,
    /// sequence or padding may be null or uninitialized. `fuel` bounds the walk
    /// on a cyclic table.
    fn nonnull_word(&self, fields: &[FieldLayout], offset: u64, fuel: usize) -> bool {
        let Some(fuel) = fuel.checked_sub(1) else {
            return false;
        };
        let holder = fields.iter().find(|f| {
            self.in_range(f.layout)
                && static_size(self.get(f.layout), false)
                    .is_some_and(|size| f.offset <= offset && offset - f.offset < size)
        });
        let Some(f) = holder else {
            return false;
        };
        let child = self.get(f.layout);
        let within = offset - f.offset;
        !f.unaligned
            && match child.kind {
                LayoutKind::Block { .. }
                | LayoutKind::Box { .. }
                | LayoutKind::Scalar(ScalarKind::Pointer)
                | LayoutKind::Slice { .. } => within == 0,
                LayoutKind::Aggregate => self.nonnull_word(&child.fields, within, fuel),
                _ => false,
            }
    }

    /// A flat record's extent with every length prefix zero is its static end.
    fn flat_extent(&mut self, s: &Subject, r: &LayoutRecord, end: Option<u64>) {
        let Some(end) = end else { return };
        if r.align != 1 {
            self.fail(
                s.clone(),
                Obligation::Alignment,
                "a flat extent has alignment 1",
            );
        }
        let prefixed = r.size
            == SizeSpec::Dynamic(vec![
                SizeTerm::Constant(FLAT_PREFIX),
                SizeTerm::ReadLength {
                    offset: 0,
                    scalar: ScalarKind::U64,
                },
                SizeTerm::Add,
            ]);
        let zeros = vec![0u8; usize::try_from(end).unwrap_or(0).saturating_add(64)];
        let expected = if prefixed { FLAT_PREFIX } else { end };
        if evaluate_size(&r.size, &zeros).ok() != Some(expected) {
            self.fail(
                s.clone(),
                Obligation::Size,
                "flat extent with empty variable fields is not the static end",
            );
        }
    }

    /// A `Shared` root block's payload: the value inline at offset 0, thawed
    /// before its own drop runs, never copied (MEM-11).
    fn frozen(&mut self, s: &Subject, r: &LayoutRecord, value: LayoutId) {
        let v = self.get(value).clone();
        let SizeSpec::Fixed(size) = v.size else {
            self.fail(
                s.clone(),
                Obligation::Size,
                "a frozen value of dynamic extent",
            );
            return;
        };
        self.exact(s, r, size, v.align);
        if r.fields
            != [FieldLayout {
                ordinal: 0,
                offset: 0,
                layout: value,
                unaligned: false,
            }]
        {
            self.fail(
                s.clone(),
                Obligation::Offset,
                "a frozen payload holds its value alone at offset 0",
            );
        }
        let scan = if v.scan.is_empty() {
            Vec::new()
        } else {
            vec![ScanStep::Inline {
                offset: 0,
                layout: value,
            }]
        };
        if r.scan != scan {
            self.fail(
                s.clone(),
                Obligation::Scan,
                "a frozen payload scans its value",
            );
        }
        let mut drop = Vec::new();
        if !v.drop.is_empty() {
            if !matches!(
                v.kind,
                LayoutKind::Handle | LayoutKind::HostCapability { .. }
            ) {
                drop.push(DropStep::Thaw {
                    offset: 0,
                    layout: value,
                });
            }
            drop.push(DropStep::Child {
                offset: 0,
                layout: value,
            });
        }
        if r.drop != drop {
            self.fail(
                s.clone(),
                Obligation::DropOrder,
                "a frozen payload thaws its counted graph, then drops its value",
            );
        }
        if v.finalizer.is_some() || r.finalizer.is_some() {
            self.fail(
                s.clone(),
                Obligation::DropOrder,
                "a frozen value holds no resource",
            );
        }
        self.copy(s, r, CopyRecipe::NotCopyable);
    }

    fn inline(&mut self, s: &Subject, r: &LayoutRecord, element: LayoutId, count: u64) {
        let e = self.get(element).clone();
        let SizeSpec::Fixed(stride) = e.size else {
            self.fail(
                s.clone(),
                Obligation::Size,
                "Inline of a dynamic-extent element",
            );
            return;
        };
        if stride.checked_mul(count).map(SizeSpec::Fixed) != Some(r.size.clone()) {
            self.fail(
                s.clone(),
                Obligation::Size,
                "Inline size is not count times the element size",
            );
        }
        let flat = r.copy == CopyRecipe::FlatBytes;
        if r.align != e.align && !(flat && r.align == 1) {
            self.fail(
                s.clone(),
                Obligation::Alignment,
                "Inline alignment is the element's",
            );
        }
        let scan = if count > 0 && !e.scan.is_empty() {
            vec![ScanStep::Repeat {
                offset: 0,
                count,
                stride,
                element,
            }]
        } else {
            Vec::new()
        };
        let drop = if count > 0 && !e.drop.is_empty() {
            vec![DropStep::RepeatReverse {
                offset: 0,
                count,
                stride,
                element,
            }]
        } else {
            Vec::new()
        };
        if r.scan != scan {
            self.fail(
                s.clone(),
                Obligation::Scan,
                "Inline scans every element with holders",
            );
        }
        if r.drop != drop {
            self.fail(
                s.clone(),
                Obligation::DropOrder,
                "Inline drops its elements from the highest index",
            );
        }
    }

    fn sequence(&mut self, s: &Subject, r: &LayoutRecord, element: LayoutId) {
        let e = self.get(element).clone();
        let SizeSpec::Fixed(stride) = e.size else {
            self.fail(
                s.clone(),
                Obligation::Size,
                "a sequence of dynamic-extent elements",
            );
            return;
        };
        let Ok(offset) = sequence_elements_offset(&e) else {
            return;
        };
        let program = SizeSpec::Dynamic(vec![
            SizeTerm::Constant(offset),
            SizeTerm::ReadLength {
                offset: 0,
                scalar: ScalarKind::U64,
            },
            SizeTerm::Constant(stride),
            SizeTerm::Multiply,
            SizeTerm::Add,
        ]);
        if r.size != program {
            self.fail(
                s.clone(),
                Obligation::Size,
                "sequence size is the elements after the u64 count and capacity",
            );
        }
        let words: Vec<(u32, u64)> = r.fields.iter().map(|f| (f.ordinal, f.offset)).collect();
        if words != [(0, 0), (1, SEQUENCE_CAPACITY_OFFSET)]
            || r.fields.iter().any(|f| {
                f.unaligned || self.get(f.layout).kind != LayoutKind::Scalar(ScalarKind::U64)
            })
        {
            self.fail(
                s.clone(),
                Obligation::Offset,
                "a sequence has a u64 count at 0 and a u64 capacity at 8",
            );
        }
        if r.align < e.align.max(8) {
            self.fail(
                s.clone(),
                Obligation::Alignment,
                "sequence payload alignment is below its element's",
            );
        }
        let scan = if e.scan.is_empty() {
            Vec::new()
        } else {
            vec![ScanStep::Sequence {
                count_offset: 0,
                offset,
                stride,
                element,
            }]
        };
        let drop = if e.drop.is_empty() {
            Vec::new()
        } else {
            vec![DropStep::ElementsReverse {
                count_offset: 0,
                offset,
                stride,
                element,
            }]
        };
        if r.scan != scan {
            self.fail(
                s.clone(),
                Obligation::Scan,
                "sequence scans each element with holders",
            );
        }
        if r.drop != drop {
            self.fail(
                s.clone(),
                Obligation::DropOrder,
                "sequence drops elements from the highest index",
            );
        }
    }

    /// MEM-13: a sequence of dynamic Packed elements stores them back to back
    /// after a u64 count and a u64 count of used bytes. Each tag-selected extent
    /// must be a constant multiple of the element's alignment that holds the
    /// u32 tag at offset 0, so a walk stays aligned and reads only the tag.
    fn sequential(&mut self, s: &Subject, r: &LayoutRecord, element: LayoutId) {
        let e = self.get(element).clone();
        // A flat list builder's elements are prefixed variable flat records,
        // each sized by its own length prefix (LIR schema revision 22).
        let prefixed = e.kind == LayoutKind::Flat
            && e.align == 1
            && e.size
                == SizeSpec::Dynamic(vec![
                    SizeTerm::Constant(8),
                    SizeTerm::ReadLength {
                        offset: 0,
                        scalar: ScalarKind::U64,
                    },
                    SizeTerm::Add,
                ])
            && matches!(e.fields.as_slice(), [f] if f.offset == 8);
        let extents = match (&e.kind, &e.size, &e.tag) {
            _ if prefixed => Some(Vec::new()),
            (LayoutKind::Packed, SizeSpec::Dynamic(program), Some(tag))
                if tag.offset == 0 && tag.form == TagForm::Ordinal =>
            {
                match program.as_slice() {
                    [
                        SizeTerm::ByTag {
                            tag_offset: 0,
                            variants,
                        },
                    ] => variants
                        .iter()
                        .map(|(ordinal, p)| match p.as_slice() {
                            [SizeTerm::Constant(n)] => Some((*ordinal, *n)),
                            _ => None,
                        })
                        .collect::<Option<Vec<_>>>(),
                    _ => None,
                }
            }
            _ => None,
        };
        let Some(extents) = extents else {
            self.fail(
                s.clone(),
                Obligation::Size,
                "a sequential sequence holds dynamic Packed elements with constant tag-selected extents",
            );
            return;
        };
        let tagged: Vec<u32> = e
            .tag
            .iter()
            .flat_map(|t| t.variants.iter().map(|v| v.ordinal))
            .collect();
        let mut ordinals: Vec<u32> = extents.iter().map(|(o, _)| *o).collect();
        ordinals.sort_unstable();
        ordinals.dedup();
        if !prefixed && ordinals.len() != extents.len()
            || !prefixed
                && ordinals != {
                    let mut t = tagged.clone();
                    t.sort_unstable();
                    t
                }
            || extents
                .iter()
                .any(|&(_, n)| n < 4 || n % u64::from(e.align) != 0)
        {
            self.fail(
                s.clone(),
                Obligation::Size,
                "each Packed extent is a distinct variant's multiple of the element alignment holding its tag",
            );
        }
        let Ok(offset) = sequential_elements_offset(&e) else {
            return;
        };
        let program = SizeSpec::Dynamic(vec![
            SizeTerm::Constant(offset),
            SizeTerm::ReadLength {
                offset: 8,
                scalar: ScalarKind::U64,
            },
            SizeTerm::Add,
        ]);
        if r.size != program {
            self.fail(
                s.clone(),
                Obligation::Size,
                "sequential size is the used bytes after the count and byte words",
            );
        }
        let words: Vec<(u32, u64)> = r.fields.iter().map(|f| (f.ordinal, f.offset)).collect();
        if words != [(0, 0), (1, 8)]
            || r.fields.iter().any(|f| {
                f.unaligned || self.get(f.layout).kind != LayoutKind::Scalar(ScalarKind::U64)
            })
        {
            self.fail(
                s.clone(),
                Obligation::Offset,
                "a sequential sequence has a u64 count at 0 and a u64 byte count at 8",
            );
        }
        if r.align < e.align.max(8) {
            self.fail(
                s.clone(),
                Obligation::Alignment,
                "sequence payload alignment is below its element's",
            );
        }
        // Packed[T] is Flat (MEM-12): its elements hold no reference or resource, so a
        // sequential sequence neither scans nor drops anything.
        if !e.scan.is_empty() || !e.drop.is_empty() || e.finalizer.is_some() {
            self.fail(
                s.clone(),
                Obligation::Scan,
                "a sequential sequence's elements hold no reference or resource",
            );
        }
        if !r.scan.is_empty() {
            self.fail(
                s.clone(),
                Obligation::Scan,
                "a sequential sequence scans nothing",
            );
        }
        if !r.drop.is_empty() {
            self.fail(
                s.clone(),
                Obligation::DropOrder,
                "a sequential sequence drops nothing",
            );
        }
        if r.copy != CopyRecipe::NotCopyable {
            self.fail(
                s.clone(),
                Obligation::Copy,
                "a sequence payload is copied only through its block",
            );
        }
    }

    /// A record element's declaration-order field columns: (quantum, data
    /// offset, row width, columns, no tag).
    #[allow(clippy::type_complexity)]
    fn soa_fields(
        &mut self,
        s: &Subject,
        e: &LayoutRecord,
    ) -> Option<(u64, u64, u64, Vec<SoaColumn>, Option<SoaTag>)> {
        let quantum = e
            .fields
            .iter()
            .map(|f| u64::from(self.get(f.layout).align))
            .max()
            .unwrap_or(1);
        let data_offset = soa_data_offset(quantum).ok()?;
        let mut columns = Vec::new();
        let mut prefix = 0u64;
        for f in &e.fields {
            let Some(stride) = static_size(self.get(f.layout), false) else {
                self.fail(
                    s.clone(),
                    Obligation::Size,
                    "SoA column of a dynamic-extent field",
                );
                return None;
            };
            columns.push(SoaColumn {
                field: f.ordinal,
                prefix,
                stride,
                layout: f.layout,
                arm: None,
            });
            prefix = prefix.saturating_add(stride);
        }
        Some((quantum, data_offset, prefix, columns, None))
    }

    /// A variant element's columns (MEM-7), derived from its row: the narrowest
    /// ordinal column, then one payload column as wide as the row past its tag
    /// word. Each arm view the record claims must be exactly its variant's
    /// fields over that column, and every arm with holders needs one, so a
    /// scan or drop reaches an arm's holders only under its own tag.
    #[allow(clippy::type_complexity)]
    fn soa_variant(
        &mut self,
        s: &Subject,
        r: &LayoutRecord,
        e: &LayoutRecord,
    ) -> Option<(u64, u64, u64, Vec<SoaColumn>, Option<SoaTag>)> {
        let tag = e.tag.as_ref()?;
        if tag.offset != 0 || tag.form != TagForm::Ordinal || e.abi.unaligned {
            self.fail(
                s.clone(),
                Obligation::Tag,
                "an @soa variant row lacks an aligned ordinal tag at offset 0",
            );
            return None;
        }
        let payload_align = tag
            .variants
            .iter()
            .flat_map(|v| &v.fields)
            .map(|f| u64::from(self.get(f.layout).align))
            .max()
            .unwrap_or(1);
        let start = 4u64.div_ceil(payload_align) * payload_align;
        let Some(stride) = static_size(e, false).and_then(|size| size.checked_sub(start)) else {
            self.fail(
                s.clone(),
                Obligation::Size,
                "an @soa variant row without a payload extent",
            );
            return None;
        };
        let (scalar, width) = match tag.variants.len() {
            n if n <= 1 << 8 => (ScalarKind::U8, 1),
            n if n <= 1 << 16 => (ScalarKind::U16, 2),
            _ => (ScalarKind::U32, 4),
        };
        let quantum = payload_align.max(width);
        let data_offset = soa_data_offset(quantum).ok()?;
        let claimed: Vec<SoaColumn> = r
            .scan
            .iter()
            .filter_map(|step| match step {
                ScanStep::Columns(c) => Some(c.columns.clone()),
                _ => None,
            })
            .chain(r.drop.iter().filter_map(|step| match step {
                DropStep::ColumnsReverse(c) => Some(c.columns.clone()),
                _ => None,
            }))
            .flatten()
            .collect();
        let scanned: Vec<u32> = e
            .scan
            .iter()
            .flat_map(|step| match step {
                ScanStep::ActiveVariant { variants, .. } => {
                    variants.iter().map(|(o, _)| *o).collect()
                }
                _ => vec![],
            })
            .collect();
        let dropped: Vec<u32> = e
            .drop
            .iter()
            .flat_map(|step| match step {
                DropStep::ActiveVariant { variants, .. } => {
                    variants.iter().map(|(o, _)| *o).collect()
                }
                _ => vec![],
            })
            .collect();
        let mut columns = Vec::new();
        for variant in &tag.variants {
            let Some(view) = claimed.iter().find(|c| c.arm == Some(variant.ordinal)) else {
                if scanned.contains(&variant.ordinal) || dropped.contains(&variant.ordinal) {
                    self.fail(
                        s.clone(),
                        Obligation::Scan,
                        "an @soa variant arm with holders has no column view",
                    );
                }
                continue;
            };
            let fields: Option<Vec<FieldLayout>> = variant
                .fields
                .iter()
                .map(|f| {
                    Some(FieldLayout {
                        offset: f.offset.checked_sub(start)?,
                        ..f.clone()
                    })
                })
                .collect();
            let arm = self.get(view.layout).clone();
            if fields.as_ref() != Some(&arm.fields)
                || arm.kind != LayoutKind::Aggregate
                || arm.tag.is_some()
                || !matches!(arm.size, SizeSpec::Fixed(size) if size <= stride)
                || u64::from(arm.align) != payload_align
            {
                self.fail(
                    s.clone(),
                    Obligation::Offset,
                    "an @soa arm view is not its variant's payload over the payload column",
                );
            }
            columns.push(SoaColumn {
                field: variant.ordinal,
                prefix: width,
                stride,
                layout: view.layout,
                arm: Some(variant.ordinal),
            });
        }
        Some((
            quantum,
            data_offset,
            width + stride,
            columns,
            Some(SoaTag { prefix: 0, scalar }),
        ))
    }

    fn soa(&mut self, s: &Subject, r: &LayoutRecord, element: LayoutId) {
        let e = self.get(element).clone();
        let derived = if e.tag.is_some() {
            self.soa_variant(s, r, &e)
        } else {
            self.soa_fields(s, &e)
        };
        let Some((quantum, data_offset, prefix, columns, tag)) = derived else {
            return;
        };
        let placement = |pick: &dyn Fn(&LayoutRecord) -> bool| SoaColumns {
            count_offset: 0,
            capacity_offset: 8,
            data_offset,
            capacity_quantum: quantum,
            columns: columns
                .iter()
                .filter(|c| pick(self.get(c.layout)))
                .cloned()
                .collect(),
            tag,
        };
        let scanned = placement(&|c| !c.scan.is_empty());
        let dropped = placement(&|c| !c.drop.is_empty());
        let scan = if scanned.columns.is_empty() {
            Vec::new()
        } else {
            vec![ScanStep::Columns(scanned)]
        };
        let drop = if dropped.columns.is_empty() {
            Vec::new()
        } else {
            vec![DropStep::ColumnsReverse(dropped)]
        };
        if r.scan != scan {
            self.fail(
                s.clone(),
                Obligation::Scan,
                "SoA scan does not cover exactly the columns with holders",
            );
        }
        if r.drop != drop {
            self.fail(
                s.clone(),
                Obligation::DropOrder,
                "SoA drop is not elements high to low, fields in reverse",
            );
        }
        if u64::from(r.align) < quantum.max(8) {
            self.fail(
                s.clone(),
                Obligation::Alignment,
                "SoA payload alignment is below its columns'",
            );
        }
        let program = SizeSpec::Dynamic(vec![
            SizeTerm::Constant(data_offset),
            SizeTerm::ReadLength {
                offset: 8,
                scalar: ScalarKind::U64,
            },
            SizeTerm::Constant(prefix),
            SizeTerm::Multiply,
            SizeTerm::Add,
        ]);
        if r.size != program {
            self.fail(
                s.clone(),
                Obligation::Size,
                "SoA size is the columns at the stored capacity",
            );
        }
    }

    /// Stack discipline of a dynamic size program.
    fn program(&mut self, s: &Subject, program: &[SizeTerm]) {
        fn depth(program: &[SizeTerm]) -> Result<(), String> {
            let mut d = 0usize;
            for term in program {
                match term {
                    SizeTerm::Constant(_) | SizeTerm::ReadLength { .. } => d += 1,
                    SizeTerm::Add | SizeTerm::Multiply => {
                        d = d
                            .checked_sub(1)
                            .filter(|&n| n >= 1)
                            .ok_or("binary step underflows")?
                    }
                    SizeTerm::AlignUp(a) => {
                        if d == 0 || !a.is_power_of_two() {
                            return Err("AlignUp needs a value and a power of two".into());
                        }
                    }
                    SizeTerm::Dup => {
                        if d == 0 {
                            return Err("Dup of an empty stack".into());
                        }
                        d += 1;
                    }
                    SizeTerm::At(inner) => {
                        if d == 0 {
                            return Err("At without an offset".into());
                        }
                        depth(inner)?;
                    }
                    SizeTerm::ByTag { variants, .. } => {
                        let mut seen: Vec<u32> = variants.iter().map(|(o, _)| *o).collect();
                        seen.sort();
                        seen.dedup();
                        if seen.len() != variants.len() || variants.is_empty() {
                            return Err("ByTag ordinals repeat or are empty".into());
                        }
                        for (_, inner) in variants {
                            depth(inner)?;
                        }
                        d += 1;
                    }
                }
            }
            if d == 1 {
                Ok(())
            } else {
                Err("program does not leave one value".into())
            }
        }
        if let Err(e) = depth(program) {
            self.fail(s.clone(), Obligation::SizeProgram, e);
        }
    }

    /// Inline containment must be acyclic: recursion passes through a block.
    fn recursion(&mut self) {
        let n = self.table.layouts.len();
        let mut state = vec![0u8; n];
        for start in 0..n {
            if state[start] != 0 {
                continue;
            }
            let mut stack = vec![(start, 0usize)];
            state[start] = 1;
            while let Some(&mut (node, ref mut next)) = stack.last_mut() {
                let children = self.inline_children(LayoutId(node as u32));
                if let Some(&child) = children.get(*next) {
                    *next += 1;
                    let c = child.0 as usize;
                    if c >= n {
                        continue;
                    }
                    match state[c] {
                        0 => {
                            state[c] = 1;
                            stack.push((c, 0));
                        }
                        1 => {
                            self.fail(
                                Subject::Layout(child),
                                Obligation::Recursion,
                                "an inline cycle never passes through a block",
                            );
                        }
                        _ => {}
                    }
                } else {
                    state[node] = 2;
                    stack.pop();
                }
            }
        }
    }

    fn inline_children(&self, id: LayoutId) -> Vec<LayoutId> {
        let r = self.get(id);
        let mut out: Vec<_> = r.fields.iter().map(|f| f.layout).collect();
        if let Some(tag) = &r.tag {
            out.extend(
                tag.variants
                    .iter()
                    .flat_map(|v| v.fields.iter().map(|f| f.layout)),
            );
        }
        match &r.kind {
            LayoutKind::Inline { element, .. }
            | LayoutKind::Simd { element, .. }
            | LayoutKind::Sequence { element }
            | LayoutKind::Sequential { element }
            | LayoutKind::SoaColumns { element } => out.push(*element),
            _ => {}
        }
        out
    }

    fn scoped_path_layout(&self, mut layout: LayoutId, fields: &[u32]) -> Option<LayoutId> {
        for field in fields {
            let Some(record) = self.table.layouts.get(layout.0 as usize) else {
                return None;
            };
            let Some(next) = record.fields.iter().find(|f| f.ordinal == *field) else {
                return None;
            };
            layout = next.layout;
        }
        self.table.layouts.get(layout.0 as usize).map(|_| layout)
    }

    fn scoped_path(&self, layout: LayoutId, fields: &[u32]) -> bool {
        self.scoped_path_layout(layout, fields).is_some_and(|id| {
            matches!(
                self.table.layouts[id.0 as usize].kind,
                LayoutKind::Callable { .. }
            )
        })
    }

    fn fn_abi(&mut self, id: FnAbiId, abi: &FnAbi) {
        let s = Subject::FnAbi(id);
        if let Err(detail) = crate::abi::check_receipt_contract(abi, self.table) {
            self.fail(s.clone(), Obligation::Abi, detail);
        }
        if abi.version != crate::ABI_VERSION || abi.key.abi_version != crate::ABI_VERSION {
            self.fail(s.clone(), Obligation::Identity, "stale ABI version");
        }
        if abi.scoped_contract
            != crate::abi::scoped_contract_digest(&abi.nonretaining, &abi.nonretaining_captures)
            || !abi.nonretaining.windows(2).all(|w| w[0] < w[1])
            || !abi.nonretaining_captures.windows(2).all(|w| w[0] < w[1])
            || abi.nonretaining.iter().any(|p| {
                abi.parameters.get(p.parameter as usize).is_none_or(|a| {
                    self.scoped_path_layout(a.layout, &p.fields).is_none()
                        || a.convention == hirsel_core_contract::Passing::Take
                        || a.convention == hirsel_core_contract::Passing::Mut
                            && !self.scoped_path(a.layout, &p.fields)
                })
            })
        {
            self.fail(
                s.clone(),
                Obligation::Abi,
                "invalid scoped nonretaining contract",
            );
        }
        if !abi
            .scoped_entries
            .windows(2)
            .all(|w| w[0].parameter < w[1].parameter)
            || abi.scoped_entries.iter().any(|e| {
                abi.parameters
                    .get(e.parameter.parameter as usize)
                    .is_none_or(|p| !self.scoped_path(p.layout, &e.parameter.fields))
            })
        {
            self.fail(
                s.clone(),
                Obligation::Abi,
                "invalid nested scoped entry selector",
            );
        }
        let foreign = abi.shape == CallableShape::Foreign;
        if abi.context == foreign {
            self.fail(
                s.clone(),
                Obligation::Abi,
                "every Hirsel entry takes the run context, and no extern does",
            );
        }
        let environment = matches!(
            abi.shape,
            CallableShape::Closure | CallableShape::IterNext | CallableShape::IterDrop
        );
        if abi.environment != environment {
            self.fail(
                s.clone(),
                Obligation::Abi,
                "only closure and iterator thunks take an environment",
            );
        }
        if environment && !abi.caller_site {
            self.fail(
                s.clone(),
                Obligation::Abi,
                "a callable ABI carries caller sites",
            );
        }
        if foreign && abi.caller_site {
            self.fail(s.clone(), Obligation::Abi, "an extern has no caller sites");
        }
        // A C function returns a number or pointer, or nothing.
        if foreign
            && (abi.destination
                || abi.result.as_ref().is_some_and(|r| {
                    !self.in_range(r.layout)
                        || crate::abi::foreign_scalar(self.table, r.layout).is_none()
                }))
        {
            self.fail(
                s.clone(),
                Obligation::Abi,
                "an extern returns a C scalar, nullable pointer or unit, with no destination",
            );
        }
        let pointer = PhysicalSlot {
            ty: PhysicalType::Pointer,
            size: self.target.scalar_size(ScalarKind::Pointer),
            align: self.target.scalar_align(ScalarKind::Pointer),
        };
        let slot = |k: ScalarKind| PhysicalSlot {
            ty: physical(k),
            size: self.target.scalar_size(k),
            align: self.target.scalar_align(k),
        };
        if let Some(parameter) = abi.result_arena {
            let valid = abi.parameters.iter().any(|p| {
                p.source == parameter
                    && p.convention == hirsel_core_contract::Passing::Borrow
                    && self.in_range(p.layout)
                    && self.get(p.layout).kind == LayoutKind::Arena
            }) && abi.result.as_ref().is_some_and(|r| {
                self.in_range(r.layout)
                    && matches!(
                        self.get(r.layout).kind,
                        LayoutKind::Block { .. } | LayoutKind::Box { .. }
                    )
            });
            if !valid {
                self.fail(
                    s.clone(),
                    Obligation::Abi,
                    "a result arena names an immutable Arena parameter and a block result",
                );
            }
        }
        if abi.diverges && (abi.result.is_some() || abi.destination) {
            self.fail(
                s.clone(),
                Obligation::Abi,
                "a diverging ABI has no result or result destination",
            );
        }
        if let Some(result) = &abi.result {
            if !self.in_range(result.layout) || self.get(result.layout).kind == LayoutKind::Unit {
                self.fail(
                    s.clone(),
                    Obligation::Abi,
                    "a result output pointer names a non-unit layout",
                );
            }
            let declared: std::collections::BTreeSet<_> =
                result.borrowed_parameters.iter().copied().collect();
            if declared.len() != result.borrowed_parameters.len()
                || declared.iter().any(|&ordinal| {
                    !abi.parameters.iter().any(|p| {
                        p.source == ordinal && p.convention == hirsel_core_contract::Passing::Borrow
                    })
                })
            {
                self.fail(
                    s.clone(),
                    Obligation::Abi,
                    "borrowed result names only distinct Borrow parameters",
                );
            }
            if result.slots != [pointer] {
                self.fail(
                    s.clone(),
                    Obligation::Abi,
                    "a result travels through one output pointer",
                );
            }
            if self.in_range(result.layout) {
                // Reconstruct the physical result independently of the producer's
                // register annotation. Never accept a forged aggregate or wide
                // integer as a one-word return, or a different scalar encoding.
                let expected = if foreign {
                    None
                } else {
                    match self.get(result.layout).kind {
                        LayoutKind::Scalar(ScalarKind::U128 | ScalarKind::I128) => None,
                        LayoutKind::Scalar(kind) => Some(kind),
                        LayoutKind::Path => Some(ScalarKind::U64),
                        LayoutKind::Block { .. }
                        | LayoutKind::Box { .. }
                        | LayoutKind::Shared { .. }
                        | LayoutKind::Header
                        | LayoutKind::Arena
                        | LayoutKind::SealedRegion { .. } => Some(ScalarKind::Pointer),
                        _ => None,
                    }
                };
                if result.register != expected {
                    self.fail(
                        s.clone(),
                        Obligation::Abi,
                        "typed register result differs from its layout",
                    );
                }
            }
        } else if abi.destination {
            self.fail(
                s.clone(),
                Obligation::Abi,
                "a caller destination without a result",
            );
        }
        for (i, p) in abi.parameters.iter().enumerate() {
            if p.source != i as u32 {
                self.fail(
                    s.clone(),
                    Obligation::Abi,
                    "parameters are in ordinal order",
                );
            }
            if !self.in_range(p.layout) {
                self.fail(
                    s.clone(),
                    Obligation::Reference,
                    "parameter layout outside the table",
                );
                continue;
            }
            let layout = self.get(p.layout);
            let expected = match (p.convention, p.passing) {
                (hirsel_core_contract::Passing::Mut, ParameterPassing::ExclusiveAddress) => {
                    Some((vec![pointer], ScalarExtension::None))
                }
                (hirsel_core_contract::Passing::Mut, _)
                | (_, ParameterPassing::ExclusiveAddress) => None,
                (_, ParameterPassing::ByValue) => match &layout.kind {
                    LayoutKind::Scalar(k) => Some((vec![slot(*k)], extension_of(*k))),
                    LayoutKind::Unit => Some((Vec::new(), ScalarExtension::None)),
                    LayoutKind::Block { .. }
                    | LayoutKind::Box { .. }
                    | LayoutKind::Shared { .. }
                    | LayoutKind::Arena
                    | LayoutKind::SealedRegion { .. } => {
                        Some((vec![pointer], ScalarExtension::None))
                    }
                    LayoutKind::Slice { .. } => Some((
                        vec![pointer, slot(ScalarKind::U64), slot(ScalarKind::U64)],
                        ScalarExtension::None,
                    )),
                    LayoutKind::Callable { .. } => Some((vec![pointer; 2], ScalarExtension::None)),
                    LayoutKind::Iterator { .. } => Some((vec![pointer; 3], ScalarExtension::None)),
                    _ => None,
                },
                (hirsel_core_contract::Passing::Borrow, ParameterPassing::ReadOnlyAddress)
                | (hirsel_core_contract::Passing::Take, ParameterPassing::TransferAddress) => {
                    (!matches!(layout.kind, LayoutKind::Scalar(_) | LayoutKind::Unit))
                        .then(|| (vec![pointer], ScalarExtension::None))
                }
                (hirsel_core_contract::Passing::Borrow, ParameterPassing::PointerLength) => {
                    let flat = layout.kind == LayoutKind::Flat
                        || matches!(layout.kind, LayoutKind::Block { payload } | LayoutKind::Box { payload } if self.in_range(payload) && self.get(payload).kind == LayoutKind::Flat);
                    flat.then(|| (vec![pointer, slot(ScalarKind::U64)], ScalarExtension::None))
                }
                _ => None,
            };
            match expected {
                Some((slots, extension)) if slots == p.slots && extension == p.extension => {}
                _ => self.fail(
                    s.clone(),
                    Obligation::Abi,
                    format!("parameter {i} passing does not match its convention and layout"),
                ),
            }
        }
    }
}

fn scan_ids(step: &ScanStep, out: &mut Vec<LayoutId>) {
    match step {
        ScanStep::Inline { layout, .. } => out.push(*layout),
        ScanStep::Sequence { element, .. } | ScanStep::Repeat { element, .. } => out.push(*element),
        ScanStep::ActiveVariant { variants, .. } => variants
            .iter()
            .flat_map(|(_, s)| s)
            .for_each(|s| scan_ids(s, out)),
        ScanStep::Columns(c) => out.extend(c.columns.iter().map(|c| c.layout)),
        ScanStep::Handle { ty, obtains, .. } => {
            out.extend(*ty);
            out.extend(*obtains);
        }
        _ => {}
    }
}

fn drop_ids(step: &DropStep, out: &mut Vec<LayoutId>) {
    match step {
        DropStep::Child { layout, .. } | DropStep::Thaw { layout, .. } => out.push(*layout),
        DropStep::ElementsReverse { element, .. } | DropStep::RepeatReverse { element, .. } => {
            out.push(*element)
        }
        DropStep::ActiveVariant { variants, .. } => variants
            .iter()
            .flat_map(|(_, s)| s)
            .for_each(|s| drop_ids(s, out)),
        DropStep::ColumnsReverse(c) => out.extend(c.columns.iter().map(|c| c.layout)),
        _ => {}
    }
}
