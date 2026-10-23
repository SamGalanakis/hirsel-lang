//! Content keys of layouts and FnAbis (design §1e, `LayoutKey`).
//!
//! The layout graph has one node per layout and one edge per layout reference a record
//! makes (its kind, its fields, its variants' fields, its scan and drop programs), in
//! that order. It can be cyclic: `Cons` holds a `Box` of `Cons`, and `Even`/`Odd` are
//! mutually recursive through a deferred Box payload (`hirsel_layout::layout`'s
//! `derive_reserved`). `hirsel_graph::scc_keys` keys it: a layout outside every cycle
//! hashes its record with each reference replaced by its target's key (a Merkle hash),
//! and a member of a cyclic component hashes the rooted DFS of its component, writing a
//! member already reached as its discovery ordinal. A record's local content is its
//! snapshot with every reference erased, so a key depends on content and field-order
//! edges only, never on `LayoutId`s, request order or unrelated layouts.
use crate::abi_types::{FnAbi, FnAbiId};
use crate::layout_types::{
    DropStep, LayoutId, LayoutKey, LayoutKind, LayoutRecord, LayoutTable, ScanStep,
};
use hirsel_syntax::ContentHash;

pub fn scan_references_mut(steps: &mut [ScanStep], f: &mut impl FnMut(&mut LayoutId)) {
    for step in steps {
        match step {
            ScanStep::Inline { layout, .. } => f(layout),
            ScanStep::Sequence { element, .. } | ScanStep::Repeat { element, .. } => f(element),
            ScanStep::ActiveVariant { variants, .. } => {
                for (_, steps) in variants {
                    scan_references_mut(steps, f);
                }
            }
            ScanStep::Columns(columns) => {
                for column in &mut columns.columns {
                    f(&mut column.layout);
                }
            }
            ScanStep::Handle { ty, obtains, .. } => {
                if let Some(ty) = ty {
                    f(ty);
                }
                if let Some(obtains) = obtains {
                    f(obtains);
                }
            }
            ScanStep::Counted { .. }
            | ScanStep::Root { .. }
            | ScanStep::Uncounted { .. }
            | ScanStep::SliceOwner { .. }
            | ScanStep::Environment { .. } => {}
        }
    }
}

impl LayoutRecord {
    /// Every layout reference the record makes, mutably, in record order: the kind's,
    /// then the fields', the variants' fields', the scan program's and the drop
    /// program's.
    pub fn for_each_reference_mut(&mut self, f: &mut impl FnMut(&mut LayoutId)) {
        match &mut self.kind {
            LayoutKind::Inline { element, .. }
            | LayoutKind::Simd { element, .. }
            | LayoutKind::Slice { element }
            | LayoutKind::Iterator { element }
            | LayoutKind::SoaColumns { element }
            | LayoutKind::Sequence { element }
            | LayoutKind::Sequential { element } => f(element),
            LayoutKind::Block { payload }
            | LayoutKind::Box { payload }
            | LayoutKind::Shared { payload } => f(payload),
            LayoutKind::SealedRegion { root: value } | LayoutKind::Frozen { value } => f(value),
            LayoutKind::Callable {
                environment: Some(environment),
            } => f(environment),
            LayoutKind::Unit
            | LayoutKind::Scalar(_)
            | LayoutKind::Aggregate
            | LayoutKind::Environment { .. }
            | LayoutKind::Variant
            | LayoutKind::Packed
            | LayoutKind::Opaque { .. }
            | LayoutKind::Handle
            | LayoutKind::HostCapability { .. }
            | LayoutKind::Path
            | LayoutKind::Arena
            | LayoutKind::Callable { environment: None }
            | LayoutKind::Resource
            | LayoutKind::Header
            | LayoutKind::PayloadReceipt { .. }
            | LayoutKind::Flat => {}
        }
        for field in &mut self.fields {
            f(&mut field.layout);
        }
        for variant in self.tag.iter_mut().flat_map(|t| &mut t.variants) {
            for field in &mut variant.fields {
                f(&mut field.layout);
            }
        }
        fn drop(steps: &mut [DropStep], f: &mut impl FnMut(&mut LayoutId)) {
            for step in steps {
                match step {
                    DropStep::Child { layout, .. } | DropStep::Thaw { layout, .. } => f(layout),
                    DropStep::ElementsReverse { element, .. }
                    | DropStep::RepeatReverse { element, .. } => f(element),
                    DropStep::ActiveVariant { variants, .. } => {
                        for (_, steps) in variants {
                            drop(steps, f);
                        }
                    }
                    DropStep::ColumnsReverse(columns) => {
                        for column in &mut columns.columns {
                            f(&mut column.layout);
                        }
                    }
                    DropStep::Finalize
                    | DropStep::Release { .. }
                    | DropStep::ReleaseRoot { .. }
                    | DropStep::ReleaseSealed { .. }
                    | DropStep::ReleaseReceipt { .. }
                    | DropStep::Environment { .. }
                    | DropStep::CloseHandle { .. } => {}
                }
            }
        }
        scan_references_mut(&mut self.scan, f);
        drop(&mut self.drop, f);
    }

    /// The layouts the record references, in record order (`for_each_reference_mut`).
    pub fn references(&self) -> Vec<LayoutId> {
        let mut out = Vec::new();
        self.clone()
            .for_each_reference_mut(&mut |id: &mut LayoutId| out.push(*id));
        out
    }
}

/// The content key of every layout of one table, by `LayoutId`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LayoutKeys {
    keys: Vec<LayoutKey>,
}

impl LayoutKeys {
    /// Key one completed layout and its reachable graph during planning, without
    /// hashing unrelated rows. Uses the same SCC encoding as the finished table.
    pub fn rooted(table: &LayoutTable, root: LayoutId) -> LayoutKey {
        let keys = hirsel_graph::scc_keys(
            &[root.0],
            |n| {
                table.layouts[n as usize]
                    .references()
                    .into_iter()
                    .map(|id| id.0)
                    .collect()
            },
            |n, w| {
                let mut record = table.layouts[n as usize].clone();
                record.for_each_reference_mut(&mut |id| *id = LayoutId(0));
                w.bytes(b"hirsel.layout.v1");
                w.bytes(&table.layout_version.to_le_bytes());
                w.bytes(hirsel_codec::to_text(&record).as_bytes());
            },
        );
        LayoutKey(keys[&root.0])
    }

    /// Keys every layout of `table`.
    pub fn new(table: &LayoutTable) -> Self {
        let nodes: Vec<u32> = (0..table.layouts.len() as u32).collect();
        let erased: Vec<String> = table
            .layouts
            .iter()
            .map(|record| {
                let mut record = record.clone();
                record.for_each_reference_mut(&mut |id: &mut LayoutId| *id = LayoutId(0));
                hirsel_codec::to_text(&record)
            })
            .collect();
        let keys = hirsel_graph::scc_keys(
            &nodes,
            |n| {
                table.layouts[n as usize]
                    .references()
                    .into_iter()
                    .map(|id| id.0)
                    .collect()
            },
            |n, w| {
                w.bytes(b"hirsel.layout.v1");
                w.bytes(&table.layout_version.to_le_bytes());
                w.bytes(erased[n as usize].as_bytes());
            },
        );
        Self {
            keys: nodes.iter().map(|n| LayoutKey(keys[n])).collect(),
        }
    }

    /// The key of `id`, which must index the keyed table.
    pub fn key(&self, id: LayoutId) -> LayoutKey {
        self.keys[id.0 as usize]
    }

    /// Every key, by `LayoutId`.
    pub fn all(&self) -> &[LayoutKey] {
        &self.keys
    }

    /// The content key of an FnAbi: the ABI with its parameter and result layouts
    /// replaced by their keys. Unlike `abi::abi_digest`, which names only the machine
    /// convention, it covers what emission reads through those layouts.
    pub fn fn_abi(&self, abi: &FnAbi) -> ContentHash {
        let mut erased = abi.clone();
        let mut references = Vec::new();
        for parameter in &mut erased.parameters {
            references.push(self.key(parameter.layout));
            parameter.layout = LayoutId(0);
        }
        if let Some(result) = &mut erased.result {
            references.push(self.key(result.layout));
            result.layout = LayoutId(0);
        }
        let text = hirsel_codec::to_text(&erased);
        ContentHash::of_parts(
            "hirsel.fn-abi.v2",
            std::iter::once(text.as_bytes()).chain(references.iter().map(|k| &k.0.0[..])),
        )
    }

    /// `fn_abi` of the table's FnAbi `id`.
    pub fn fn_abi_of(&self, table: &LayoutTable, id: FnAbiId) -> Option<ContentHash> {
        table.fn_abis.get(id.0 as usize).map(|abi| self.fn_abi(abi))
    }
}
