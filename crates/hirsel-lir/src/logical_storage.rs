//! The logical storage charged for private Map entry lists. Build the index
//! once with a frozen layout table; no charge site scans the table again.
use crate::layout::{logical_size, sequence_elements_offset};
use crate::layout_types::*;
use std::collections::BTreeMap;

#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct LiveEntries {
    pub pair: LayoutId,
    pub logical: u64,
    pub tag: TagEncoding,
    pub first: u64,
    pub stride: u64,
}
impl LiveEntries {
    /// The native count program: element start, stride, tag offset, form, bit.
    pub fn program(&self) -> [u64; 5] {
        let (form, bit) = match self.tag.form {
            TagForm::Ordinal => (0, 0),
            TagForm::LiveBit { bit } => (1, u64::from(bit)),
            TagForm::Niche {
                nullary: 0,
                dataful: 1,
            } => (2, 0),
            _ => unreachable!("Map optional-entry ordinals"),
        };
        [self.first, self.stride, self.tag.offset, form, bit]
    }
}
#[derive(Clone, Debug, Default, Eq, PartialEq, Hash)]
pub struct MapStorage {
    entries: BTreeMap<LayoutId, LiveEntries>,
}
impl MapStorage {
    pub fn new(table: &LayoutTable) -> Self {
        let mut entries = BTreeMap::new();
        for owner in &table.layouts {
            if !matches!(&owner.kind, LayoutKind::Opaque { owner } if *owner == hirsel_syntax::DefPath::core("Map"))
            {
                continue;
            }
            let Some(entry_field) = owner.fields.first() else {
                continue;
            };
            let Some(LayoutRecord {
                kind: LayoutKind::Block { payload },
                ..
            }) = table.layout(entry_field.layout)
            else {
                continue;
            };
            let Some(LayoutRecord {
                kind: LayoutKind::Sequence { element },
                ..
            }) = table.layout(*payload)
            else {
                continue;
            };
            let Some(entry) = table.layout(*element) else {
                continue;
            };
            let Some(tag) = &entry.tag else {
                continue;
            };
            let Some(pair) = tag
                .variants
                .iter()
                .find(|v| v.ordinal == 1)
                .and_then(|v| v.fields.first())
            else {
                continue;
            };
            let (Some(logical), SizeSpec::Fixed(stride), Ok(first)) = (
                logical_size(table, pair.layout),
                &entry.size,
                sequence_elements_offset(entry),
            ) else {
                continue;
            };
            entries.insert(
                *payload,
                LiveEntries {
                    pair: pair.layout,
                    logical,
                    tag: tag.clone(),
                    first,
                    stride: *stride,
                },
            );
        }
        Self { entries }
    }
    pub fn entries(&self, payload: LayoutId) -> Option<&LiveEntries> {
        self.entries.get(&payload)
    }
}
