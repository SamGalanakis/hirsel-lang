//! Debug intervals (design §1e): where each function's machine code came from, as a
//! backend records it beside a unit's object for the link-time debug object. Units carry
//! no DWARF and no source paths. An interval names an item by its definition and a byte
//! offset relative to the item, so a moved item keeps its unit's object and intervals; each
//! link resolves them against the current sources.
//!
//! Items are named by `DefPath`, not by a `hirsel_span::Anchor` as design §3 spells it:
//! intervals are stored with their unit's object, and anchors are process-local.
use crate::profile::ProfileSources;
use hirsel_syntax::DefPath;

/// One function's intervals.
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct DebugFunction {
    /// The function's link name.
    pub symbol: String,
    /// Its code's size in bytes.
    pub size: u64,
    /// The items its offsets are relative to; the first is its own definition. Code a
    /// backend inlined from another unit is relative to that unit's definition.
    pub items: Vec<DefPath>,
    /// Its declaration's offset in the first item.
    pub declaration: u32,
    /// Mapped code, in address order and disjoint. Code outside every range (between two
    /// ranges, before the first or after the last) maps to no source: the debug object ends
    /// a line sequence before it, so it never inherits the line of the code before it.
    pub ranges: Vec<DebugRange>,
}

/// Code `[start, stop)` (offsets from the function's start) came from `offset` in
/// `items[item]`.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub struct DebugRange {
    pub start: u64,
    pub stop: u64,
    pub item: u32,
    pub offset: u32,
}

impl DebugFunction {
    /// Its ranges from rows `(start, stop, item, offset)` in address order: empty rows are
    /// dropped, and adjacent rows from one site merge.
    pub fn from_rows(
        symbol: String,
        size: u64,
        items: Vec<DefPath>,
        declaration: u32,
        rows: impl IntoIterator<Item = (u64, u64, u32, u32)>,
    ) -> Self {
        let mut ranges: Vec<DebugRange> = Vec::new();
        for (start, stop, item, offset) in rows {
            if start >= stop {
                continue;
            }
            match ranges.last_mut() {
                Some(last) if last.stop == start && (last.item, last.offset) == (item, offset) => {
                    last.stop = stop
                }
                _ => ranges.push(DebugRange {
                    start,
                    stop,
                    item,
                    offset,
                }),
            }
        }
        Self {
            symbol,
            size,
            items,
            declaration,
            ranges,
        }
    }

    /// Whether the ranges are in address order, disjoint, inside the function and name an
    /// item it lists.
    pub fn well_formed(&self) -> bool {
        !self.items.is_empty()
            && self.ranges.iter().all(|r| {
                r.start < r.stop && r.stop <= self.size && (r.item as usize) < self.items.len()
            })
            && self.ranges.windows(2).all(|w| w[0].stop <= w[1].start)
    }
}

/// What a backend records beside a unit's code.
#[derive(Clone, Copy, Debug, Default)]
pub struct Profile<'a> {
    /// Record each function's debug intervals (`ObjectArtifact::debug`).
    pub debug: bool,
    /// The resolved sources measurement instrumentation embeds in code, which it needs
    /// (`EmitInputs::measurement`); without them a backend instruments nothing.
    pub sources: Option<&'a ProfileSources>,
}

hirsel_codec::snapshot_struct!(DebugRange {
    start,
    stop,
    item,
    offset
});
hirsel_codec::snapshot_struct!(DebugFunction {
    symbol,
    size,
    items,
    declaration,
    ranges
});
