//! Native projection of the existing holder scan, independent of destruction.
//!
//! An atomic or task handle has a scan even when it has no drop program. Each
//! instruction is eight u64 words. Layout operands are patched to the linked
//! registry by emission; they are never process addresses or local table IDs.
//! Unsupported borrowed/variable-flat representations explicitly yield unknown.

use crate::layout_types::{
    HandleKind, LayoutId, LayoutKind, LayoutRecord, ScalarKind, ScanStep, SizeSpec, SizeTerm,
    TagForm,
};

pub use hirsel_core_contract::handle_scan::*;

#[derive(Clone, Debug, Default, Eq, PartialEq, Hash)]
pub struct Program {
    words: Vec<u64>,
    references: Vec<(usize, LayoutId)>,
}

impl Program {
    pub fn lower(record: &LayoutRecord) -> Self {
        Self::lower_with(record, None)
    }

    pub fn lower_in(table: &crate::layout_types::LayoutTable, record: &LayoutRecord) -> Self {
        Self::lower_with(record, Some(table))
    }

    fn lower_with(record: &LayoutRecord, table: Option<&crate::layout_types::LayoutTable>) -> Self {
        let mut out = Self::default();
        if matches!(record.kind, LayoutKind::Sequential { .. }) {
            out.instruction(UNKNOWN, 0);
        } else {
            out.steps(&record.scan);
        }
        // Type closure follows layout structure even when a pointer's target
        // is not in the current value image. These edges never read a value.
        let mut references: Vec<_> = record.fields.iter().map(|f| f.layout).collect();
        if let Some(tag) = &record.tag {
            references.extend(
                tag.variants
                    .iter()
                    .flat_map(|v| v.fields.iter().map(|f| f.layout)),
            );
        }
        match record.kind {
            LayoutKind::Block { payload }
            | LayoutKind::Box { payload }
            | LayoutKind::Shared { payload } => references.push(payload),
            LayoutKind::Frozen { value } => references.push(value),
            LayoutKind::Inline { element, .. }
            | LayoutKind::Simd { element, .. }
            | LayoutKind::Slice { element }
            | LayoutKind::Sequence { element }
            | LayoutKind::Sequential { element }
            | LayoutKind::SoaColumns { element } => references.push(element),
            LayoutKind::Callable {
                environment: Some(environment),
            } => references.push(environment),
            LayoutKind::Callable { environment: None } | LayoutKind::Iterator { .. } => {
                out.instruction(TYPE_UNKNOWN, 0);
            }
            _ => {}
        }
        references.sort();
        references.dedup();
        for id in references {
            let at = out.instruction(TYPE_EDGE, 0);
            out.reference(at + 2, id);
        }
        out.value(record, table);
        out
    }

    fn value(&mut self, record: &LayoutRecord, table: Option<&crate::layout_types::LayoutTable>) {
        let at = self.instruction(VALUE, VALUE_UNKNOWN);
        // Mirror codec.rs's Flat/Sequential/Columns projection. Size and
        // placement come from the existing layout table, not carrier bytes.
        let minimum = match record.size {
            SizeSpec::Fixed(n) => n,
            _ => table
                .and_then(|t| crate::rt_program::minimum_extent(t, record).ok())
                .unwrap_or(0),
        };
        if let Some(table) = table {
            match &record.kind {
                LayoutKind::Flat => {
                    self.words[at + 2] = minimum;
                    if matches!(&record.size, SizeSpec::Dynamic(p) if matches!(p.as_slice(),
                        [SizeTerm::Constant(8), SizeTerm::ReadLength { offset: 0, scalar: ScalarKind::U64 }, SizeTerm::Add]))
                    {
                        if let [field] = record.fields.as_slice() {
                            self.words[at + 1] = VALUE_FLAT_SEQUENCE;
                            self.words[at + 3] = field.offset;
                            self.reference(at + 4, field.layout);
                            if let Some(child) = table.layout(field.layout) {
                                self.words[at + 5] = u64::from(
                                    matches!(child.size, SizeSpec::Dynamic(_)) && field.offset == 8,
                                );
                                if let SizeSpec::Fixed(stride) = child.size {
                                    self.words[at + 6] = stride;
                                }
                            }
                        }
                    } else {
                        self.words[at + 1] = VALUE_FLAT_FIELDS;
                        for field in &record.fields {
                            self.value_field(field, u64::MAX);
                        }
                    }
                    return;
                }
                LayoutKind::Packed => {
                    let Some(tag) = &record.tag else { return };
                    if tag.offset != 0 || tag.form != TagForm::Ordinal {
                        return;
                    }
                    self.words[at + 1] = VALUE_PACKED;
                    self.words[at + 2] = minimum;
                    self.words[at + 3] = u64::from(record.align);
                    self.words[at + 4] = u64::from(record.align == 1);
                    self.words[at + 5] = record.size.storage().unwrap_or(u64::MAX);
                    self.words[at + 6] = tag.variants.len() as u64;
                    for arm in &tag.variants {
                        for field in &arm.fields {
                            self.value_field(field, u64::from(arm.ordinal));
                        }
                    }
                    return;
                }
                LayoutKind::Sequential { element } => {
                    let Some(e) = table.layout(*element) else {
                        return;
                    };
                    let align = u64::from(e.align);
                    if align == 0 || !align.is_power_of_two() {
                        return;
                    }
                    self.words[at + 1] = VALUE_SEQUENTIAL;
                    self.words[at + 2] = minimum;
                    self.words[at + 3] = (16 + align - 1) & !(align - 1);
                    self.reference(at + 4, *element);
                    return;
                }
                LayoutKind::SoaColumns { element } => {
                    let Some(e) = table.layout(*element) else {
                        return;
                    };
                    let quantum = e
                        .fields
                        .iter()
                        .filter_map(|f| table.layout(f.layout).map(|r| u64::from(r.align)))
                        .max()
                        .unwrap_or(1);
                    if quantum == 0 || !quantum.is_power_of_two() {
                        return;
                    }
                    self.words[at + 1] = VALUE_COLUMNS;
                    self.words[at + 2] = minimum;
                    self.words[at + 3] = (16 + quantum - 1) & !(quantum - 1);
                    self.words[at + 4] = quantum;
                    let mut prefix = 0;
                    for f in &e.fields {
                        let Some(SizeSpec::Fixed(stride)) = table.layout(f.layout).map(|r| &r.size)
                        else {
                            return;
                        };
                        let column = self.instruction(VALUE_COLUMN, prefix);
                        self.words[column + 2] = *stride;
                        self.reference(column + 3, f.layout);
                        prefix += stride;
                    }
                    return;
                }
                _ => {}
            }
        }
        let SizeSpec::Fixed(size) = record.size else {
            if let (LayoutKind::Sequence { element }, SizeSpec::Dynamic(size)) =
                (&record.kind, &record.size)
                && let [
                    SizeTerm::Constant(start),
                    SizeTerm::ReadLength {
                        offset: 0,
                        scalar: ScalarKind::U64,
                    },
                    SizeTerm::Constant(stride),
                    SizeTerm::Multiply,
                    SizeTerm::Add,
                ] = size.as_slice()
            {
                self.words[at + 1] = VALUE_SEQUENCE;
                // Only the two fixed header fields belong to the minimum extent;
                // over-aligned elements may start later.
                self.words[at + 2] = 16;
                self.words[at + 3] = *stride;
                self.words[at + 4] = *start;
                self.reference(at + 5, *element);
            }
            return;
        };
        self.words[at + 2] = size;
        match &record.kind {
            LayoutKind::Scalar(scalar) if *scalar != ScalarKind::Pointer => {
                self.words[at + 1] = if *scalar == ScalarKind::Bool {
                    VALUE_BOOL
                } else {
                    VALUE_BITS
                };
            }
            LayoutKind::Unit | LayoutKind::Aggregate | LayoutKind::Frozen { .. } => {
                self.words[at + 1] = VALUE_FIELDS;
                for field in &record.fields {
                    self.value_field(field, u64::MAX);
                }
            }
            LayoutKind::Environment { movable } => {
                let Some(live) = record.fields.last() else {
                    return;
                };
                self.words[at + 1] = VALUE_ENVIRONMENT;
                self.words[at + 3] = live.offset;
                self.words[at + 4] = movable.len() as u64;
                for field in &record.fields[..record.fields.len() - 1] {
                    let bit = movable
                        .iter()
                        .position(|&ordinal| ordinal == field.ordinal)
                        .map_or(u64::MAX, |bit| bit as u64);
                    self.value_field(field, bit);
                }
            }
            LayoutKind::Callable { environment } if size == 16 => {
                self.words[at + 1] = VALUE_CALLABLE;
                // A generic nested function has a null code word. Its concrete
                // closure key includes the definition/body and capture types.
                if environment.is_some()
                    && let Some(identity) = record.key.instance
                {
                    self.words[at + 3] = 1;
                    for (i, bytes) in identity.0.as_chunks::<8>().0.iter().enumerate() {
                        self.words[at + 4 + i] = u64::from_le_bytes(*bytes);
                    }
                }
            }
            LayoutKind::Iterator { .. } if size == 24 => {
                self.words[at + 1] = VALUE_ITERATOR;
            }
            LayoutKind::Opaque { owner }
                if [
                    hirsel_syntax::DefPath::core("Map"),
                    hirsel_syntax::DefPath::core("Set"),
                ]
                .contains(owner)
                    && record.fields.len() == 4
                    && record
                        .fields
                        .iter()
                        .enumerate()
                        .all(|(i, f)| f.ordinal as usize == i) =>
            {
                // The codec's core Map/Set representation: entries, index,
                // cached hashes, live count. Only live entries are semantic.
                self.words[at + 1] = VALUE_ENTRIES;
                self.words[at + 3] = record.fields[0].offset;
                self.reference(at + 4, record.fields[0].layout);
                self.words[at + 5] = record.fields[3].offset;
                self.reference(at + 6, record.fields[3].layout);
            }
            LayoutKind::Opaque { owner }
                if *owner == hirsel_syntax::DefPath::core("SortedMap")
                    && record.fields.len() == 1 =>
            {
                self.words[at + 1] = VALUE_FIELDS;
                self.value_field(&record.fields[0], u64::MAX);
            }
            LayoutKind::Inline { element, count } if *count <= 65_536 => {
                let stride = if *count == 0 { 0 } else { size / count };
                if stride.checked_mul(*count) != Some(size) {
                    return;
                }
                self.words[at + 1] = VALUE_FIELDS;
                for index in 0..*count {
                    let field = self.instruction(VALUE_FIELD, index * stride);
                    self.reference(field + 2, *element);
                    self.words[field + 3] = u64::MAX;
                }
            }
            LayoutKind::Simd { element, lanes }
                if record.abi.scalars.len() == *lanes as usize && *lanes <= 65_536 =>
            {
                // SIMD's extent includes tail alignment. The authoritative
                // lane offsets omit that padding, unlike size / lanes.
                self.words[at + 1] = VALUE_FIELDS;
                for &(offset, _) in &record.abi.scalars {
                    let field = self.instruction(VALUE_FIELD, offset);
                    self.reference(field + 2, *element);
                    self.words[field + 3] = u64::MAX;
                }
            }
            LayoutKind::Variant => {
                let Some(tag) = &record.tag else { return };
                let (form, nullary) = match tag.form {
                    TagForm::Ordinal => (0, 0),
                    TagForm::Niche { nullary, dataful }
                        if nullary < 2 && dataful == 1 - nullary =>
                    {
                        (1, u64::from(nullary))
                    }
                    _ => return,
                };
                if !tag
                    .variants
                    .iter()
                    .enumerate()
                    .all(|(i, arm)| arm.ordinal as usize == i)
                {
                    return;
                }
                self.words[at + 1] = VALUE_VARIANT;
                self.words[at + 3] = tag.offset;
                self.words[at + 4] = form;
                self.words[at + 5] = nullary;
                self.words[at + 6] = tag.variants.len() as u64;
                for arm in &tag.variants {
                    for field in &arm.fields {
                        self.value_field(field, u64::from(arm.ordinal));
                    }
                }
            }
            LayoutKind::Block { payload }
            | LayoutKind::Box { payload }
            | LayoutKind::Shared { payload } => {
                self.words[at + 1] = VALUE_INDIRECT;
                self.reference(at + 3, *payload);
            }
            LayoutKind::Slice { element } if size == 24 => {
                self.words[at + 1] = VALUE_SLICE;
                self.reference(at + 3, *element);
            }
            LayoutKind::Path if size == 8 => {
                self.words[at + 1] = VALUE_PATH;
            }
            LayoutKind::Handle => {
                if let [
                    ScanStep::Handle {
                        offset: 0, kind, ..
                    },
                ] = record.scan.as_slice()
                    && *kind != HandleKind::Unknown
                {
                    self.words[at + 1] = VALUE_HANDLE;
                    self.words[at + 3] = kind.code();
                }
            }
            _ => {}
        }
    }

    fn value_field(&mut self, field: &crate::layout_types::FieldLayout, arm: u64) {
        let at = self.instruction(VALUE_FIELD, field.offset);
        self.reference(at + 2, field.layout);
        self.words[at + 3] = arm;
    }

    pub fn referenced(&self) -> impl Iterator<Item = LayoutId> + '_ {
        self.references.iter().map(|&(_, id)| id)
    }

    /// None means an operand has no executable linked descriptor. Emission
    /// must retain unknown rather than certify an empty capture in that case.
    pub fn linked(&self, index: impl Fn(LayoutId) -> Option<u32>) -> Option<Vec<u64>> {
        let mut words = self.words.clone();
        for &(at, id) in &self.references {
            words[at] = u64::from(index(id)?);
        }
        Some(words)
    }

    fn instruction(&mut self, opcode: u64, offset: u64) -> usize {
        let at = self.words.len();
        self.words.extend([opcode, offset, 0, 0, 0, 0, 0, 0]);
        at
    }

    fn reference(&mut self, at: usize, id: LayoutId) {
        self.references.push((at, id));
    }

    fn steps(&mut self, steps: &[ScanStep]) {
        for step in steps {
            match step {
                ScanStep::Handle {
                    offset,
                    kind,
                    ty,
                    obtains,
                } => {
                    if let (kind, Some(ty)) = (kind, ty)
                        && *kind != HandleKind::Unknown
                    {
                        let at = self.instruction(TYPED_HANDLE, *offset);
                        self.words[at + 2] = kind.code();
                        self.reference(at + 3, *ty);
                        if let Some(payload) = obtains {
                            self.reference(at + 4, *payload);
                        }
                    } else {
                        self.instruction(HANDLE, *offset);
                    }
                }
                ScanStep::Counted { offset }
                | ScanStep::SliceOwner { offset }
                | ScanStep::Environment { offset } => {
                    self.instruction(BLOCK, *offset);
                }
                ScanStep::Root { offset } => {
                    self.instruction(ROOT, *offset);
                }
                ScanStep::Uncounted { .. } => {
                    self.instruction(UNKNOWN, 0);
                }
                ScanStep::Inline { offset, layout } => {
                    let at = self.instruction(INLINE, *offset);
                    self.reference(at + 2, *layout);
                }
                ScanStep::ActiveVariant {
                    tag_offset,
                    form,
                    variants,
                } => {
                    for (ordinal, steps) in variants {
                        let opcode = match form {
                            TagForm::Ordinal => TAG,
                            TagForm::LiveBit { .. } => LIVE_BIT,
                            TagForm::Niche { dataful, .. } if ordinal == dataful => NONNULL,
                            TagForm::Niche { .. } => {
                                if !steps.is_empty() {
                                    self.instruction(UNKNOWN, 0);
                                }
                                continue;
                            }
                        };
                        let at = self.instruction(opcode, *tag_offset);
                        self.words[at + 2] = u64::from(*ordinal);
                        if let TagForm::LiveBit { bit } = form {
                            self.words[at + 4] = u64::from(*bit);
                        }
                        self.steps(steps);
                        self.words[at + 3] = (self.words.len() - at - WORDS) as u64;
                    }
                }
                ScanStep::Sequence {
                    count_offset,
                    offset,
                    stride,
                    element,
                } => {
                    let at = self.instruction(SEQUENCE, *offset);
                    self.words[at + 2] = *count_offset;
                    self.words[at + 3] = *stride;
                    self.reference(at + 4, *element);
                }
                ScanStep::Repeat {
                    count,
                    offset,
                    stride,
                    element,
                } => {
                    let at = self.instruction(REPEAT, *offset);
                    self.words[at + 2] = *count;
                    self.words[at + 3] = *stride;
                    self.reference(at + 4, *element);
                }
                ScanStep::Columns(columns) => {
                    for column in &columns.columns {
                        let at = self.instruction(COLUMN, columns.data_offset);
                        self.words[at + 2] = columns.count_offset;
                        self.words[at + 3] = columns.capacity_offset;
                        self.words[at + 4] = column.stride;
                        self.words[at + 5] = column.prefix;
                        self.reference(at + 6, column.layout);
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn linking_replaces_every_layout_operand_including_nested_variants() {
        let mut p = Program::default();
        p.steps(&[ScanStep::ActiveVariant {
            tag_offset: 4,
            form: TagForm::Ordinal,
            variants: vec![(
                1,
                vec![ScanStep::Inline {
                    offset: 8,
                    layout: LayoutId(91),
                }],
            )],
        }]);
        assert_eq!(p.referenced().collect::<Vec<_>>(), vec![LayoutId(91)]);
        assert!(p.linked(|_| None).is_none());
        let words = p.linked(|id| (id == LayoutId(91)).then_some(7)).unwrap();
        assert_eq!(&words[..4], &[TAG, 4, 1, WORDS as u64]);
        assert_eq!(&words[WORDS..WORDS + 3], &[INLINE, 8, 7]);
    }

    #[test]
    fn nonresource_handles_are_present_and_borrowed_pointers_are_unknown() {
        let mut p = Program::default();
        p.steps(&[
            ScanStep::Handle {
                offset: 0,
                kind: crate::layout_types::HandleKind::Unknown,
                ty: None,
                obtains: None,
            },
            ScanStep::Uncounted { offset: 8 },
        ]);
        let words = p.linked(|_| None).unwrap();
        assert_eq!(words[0], HANDLE);
        assert_eq!(words[WORDS], UNKNOWN);
    }
}
