//! Common layout lookups built once for an immutable table. Like rustc's
//! CommonTypes (compiler/rustc_middle/src/ty/context.rs:306-328,421-468), a
//! compilation resolves its common types before function workers need them.
//! Hirsel indexes existing physical rows, preserving the first-match semantics
//! of the old scans. The borrow prevents mutations while the index is used.
use crate::layout_types::{LayoutId, LayoutKind, LayoutTable, ScalarKind};
use std::collections::BTreeMap;

#[derive(Debug)]
pub struct LayoutIndex<'a> {
    table: &'a LayoutTable,
    scalars: BTreeMap<ScalarKind, LayoutId>,
    sequences: BTreeMap<LayoutId, LayoutId>,
    header: Option<LayoutId>,
}

impl<'a> LayoutIndex<'a> {
    pub fn new(table: &'a LayoutTable) -> Self {
        let mut out = Self {
            table,
            scalars: BTreeMap::new(),
            sequences: BTreeMap::new(),
            header: None,
        };
        for (i, row) in table.layouts.iter().enumerate() {
            let id = LayoutId(i as u32);
            match row.kind {
                LayoutKind::Scalar(kind) => {
                    out.scalars.entry(kind).or_insert(id);
                }
                LayoutKind::Sequence { element } => {
                    out.sequences.entry(element).or_insert(id);
                }
                LayoutKind::Header => {
                    out.header.get_or_insert(id);
                }
                _ => {}
            }
        }
        out
    }
    pub fn table(&self) -> &'a LayoutTable {
        self.table
    }
    pub fn matches(&self, table: &LayoutTable) -> bool {
        std::ptr::eq(self.table, table)
    }
    pub fn scalar(&self, kind: ScalarKind) -> Option<LayoutId> {
        self.scalars.get(&kind).copied()
    }
    pub fn header(&self) -> Option<LayoutId> {
        self.header
    }
    pub fn sequence(&self, element: LayoutId) -> Option<LayoutId> {
        self.sequences.get(&element).copied()
    }

    /// The implicit owner layout read when code consumes a slice. Its storage
    /// has its own descriptor symbol/key, so it is a dependency even though
    /// the Slice record names only the element.
    pub fn implicit(&self, id: LayoutId) -> Option<LayoutId> {
        match self.table.layout(id)?.kind {
            LayoutKind::Slice { element } => self.sequence(element),
            _ => None,
        }
    }

    /// Common rows the passes may introduce after the Value object key is
    /// computed. The passes synthesize Header, Bool and U64; other scalar rows must
    /// already be named by the unit.
    pub fn introduced(&self) -> impl Iterator<Item = LayoutId> + '_ {
        self.header.into_iter().chain(
            [ScalarKind::Bool, ScalarKind::U64]
                .into_iter()
                .filter_map(|kind| self.scalar(kind)),
        )
    }
}
