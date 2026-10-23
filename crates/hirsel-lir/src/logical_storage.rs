//! The logical storage charged for private Map entry lists. Build the index
//! once with a frozen layout table; no charge site scans the table again.
use crate::layout::{logical_size, sequence_elements_offset};
use crate::layout_types::*;
use hirsel_codec::{snapshot_enum, snapshot_struct};
use std::collections::BTreeMap;

/// Exact MEM-6 size; overflow is larger than every representable capacity.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum LogicalBytes {
    Bytes(u64),
    Overflow,
}

/// Storage facts supplement the shared declaration-derived logical graph.
/// They never replace its edges or identify a physical layout.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum StorageClass {
    Inline,
    InlineSequence {
        count: u64,
    },
    Block,
    Bytes,
    /// Independent MEM-11 root: storage walks stop here, even though the
    /// structural-operation view transparently reaches its value.
    Shared,
    Transparent,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub struct StorageFact {
    pub bytes: LogicalBytes,
    pub class: StorageClass,
}

#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct LogicalStorageGraph {
    pub shape: crate::structural::ShapeGraph,
    /// One independently derived fact per shape node, in the same order.
    pub facts: Vec<StorageFact>,
}

impl LogicalStorageGraph {
    pub fn check(&self) -> Result<(), &'static str> {
        use crate::structural::Shape;
        self.shape.check()?;
        if self.facts.len() != self.shape.nodes.len() {
            return Err("logical storage facts do not cover the shared shape graph");
        }
        for (shape, fact) in self.shape.nodes.iter().zip(&self.facts) {
            let compatible = match fact.class {
                StorageClass::Inline => {
                    matches!(shape, Shape::Leaf | Shape::Fields(_) | Shape::Variant(_))
                }
                StorageClass::InlineSequence { .. } => matches!(shape, Shape::Sequence(_)),
                StorageClass::Block => matches!(
                    shape,
                    Shape::Box(_) | Shape::Sequence(_) | Shape::Map { .. } | Shape::Set(_)
                ),
                StorageClass::Bytes => matches!(shape, Shape::Leaf),
                StorageClass::Shared => matches!(shape, Shape::Transparent(_)),
                StorageClass::Transparent => {
                    matches!(shape, Shape::Transparent(_) | Shape::Distinct(_))
                }
            };
            if !compatible {
                return Err("logical storage classification disagrees with shared shape");
            }
        }
        Ok(())
    }
}

snapshot_enum!(LogicalBytes { Bytes(bytes), Overflow });
snapshot_enum!(StorageClass { Inline, InlineSequence { count }, Block, Bytes, Shared, Transparent });
snapshot_struct!(StorageFact { bytes, class });
snapshot_struct!(LogicalStorageGraph { shape, facts });

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
            if !matches!(&owner.kind, LayoutKind::Opaque { owner } if [hirsel_syntax::DefPath::core("Map"), hirsel_syntax::DefPath::core("Set")].contains(owner))
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
