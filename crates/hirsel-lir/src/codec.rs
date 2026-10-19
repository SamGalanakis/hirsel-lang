//! The canonical SER-2 codec over native layouts (LIR schema revision 29).
//!
//! A `CodecPlan` pairs one concrete checked schema with the layouts its values
//! have: every node names the layout of the value it reads or writes, and its
//! children are the layouts' own fields, arms and elements. The plan carries
//! the schema's descriptor bytes, which an envelope must match exactly; it
//! holds no type, name or TypeId. `check` is the plan's independent agreement
//! with a layout table; `program` lowers it to the runtime's codec program,
//! the static data `hirsel_rt_v2_codec_encode` and `_decode` walk.
use crate::abi_types::{AbiParameter, FnAbiId, ParameterPassing};
use crate::ir::SymbolId;
use crate::layout::{sequence_elements_offset, sequential_elements_offset, soa_data_offset};
use crate::layout_types::{
    FieldLayout, LayoutId, LayoutKind, LayoutRecord, LayoutTable, ScalarKind, SizeSpec, TagForm,
};
use hirsel_codec::{snapshot_enum, snapshot_struct};
use hirsel_core_contract::Passing;

/// Index into `CodecPlan::nodes`.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Ord, PartialOrd)]
pub struct CodecNodeId(pub u32);

impl CodecNodeId {
    pub const fn index(self) -> usize {
        self.0 as usize
    }
}

impl hirsel_codec::Snapshot for CodecNodeId {
    fn write(&self, w: &mut hirsel_codec::Writer) {
        w.uint(u128::from(self.0));
    }
    fn read(r: &mut hirsel_codec::Reader<'_>) -> Result<Self, hirsel_codec::DecodeError> {
        Ok(Self(<u32 as hirsel_codec::Snapshot>::read(r)?))
    }
}

/// One schema node at the layout of the values it describes. Children are in
/// declaration order: field `i` of a `Fields` node is the layout's field of
/// ordinal `i`, arm `i` of a `Variant` its variant of ordinal `i`.
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub enum CodecNode {
    /// `bool`, a fixed-width integer or a float: a scalar layout.
    Scalar(LayoutId),
    /// `String` (`utf8`) or `Bytes`: a byte `Slice` layout, whose owner is a
    /// block of the byte `Sequence` payload `storage`.
    Text {
        layout: LayoutId,
        storage: LayoutId,
        utf8: bool,
    },
    /// A String/Bytes field in FlatFormatV0: byte extent followed by bytes.
    FlatText { layout: LayoutId, utf8: bool },
    /// A variable flat field: byte extent followed by the represented value.
    Framed {
        layout: LayoutId,
        value: CodecNodeId,
    },
    /// Unit, a tuple, a record, or a nominal's one-field wrapper: every field of
    /// a `Unit`, `Aggregate` or `Flat` layout, concatenated.
    Fields {
        layout: LayoutId,
        fields: Vec<CodecNodeId>,
    },
    /// A variant or `Packed[T]`: the `u32` ordinal, then the active arm's fields.
    Variant {
        layout: LayoutId,
        arms: Vec<Vec<CodecNodeId>>,
    },
    /// `box T` or a variable flat block: only the payload's encoding.
    Box {
        layout: LayoutId,
        payload: CodecNodeId,
    },
    /// `List[T]` or `Array[T]`: ordinary, sequential or SoA block storage,
    /// or an inline flat field. The wire is a u64 count followed by elements.
    Sequence {
        layout: LayoutId,
        element: CodecNodeId,
    },
    /// `Inline[T, n]` or `Simd[T, n]`: exactly `n` elements, no count.
    Repeat {
        layout: LayoutId,
        element: CodecNodeId,
    },
    /// `Map[K, V]`: live `(K, V)` pairs in insertion order; private holes
    /// and hash metadata never appear on the wire. No duplicate key encoding.
    Map {
        layout: LayoutId,
        key: CodecNodeId,
        value: CodecNodeId,
    },
    /// `Set[T]`: a container block whose entries are encoded strictly
    /// increasing by their canonical bytes.
    Set {
        layout: LayoutId,
        element: CodecNodeId,
    },
    /// `char`: a `u32` scalar layout; decoding admits only a Unicode scalar
    /// value.
    Char(LayoutId),
    /// A core `Path`: a run-context path identity, encoded as its text (a
    /// `String`); decoding admits any UTF-8 text and interns it as `path.of`.
    Path(LayoutId),
    /// A checked opaque type (SER-1, LIR schema revision 39): `value`, its
    /// representation at the same layout, is the whole encoding. Decoding then
    /// calls `CodecPlan::checks[check]` on every reconstructed occurrence, inner
    /// occurrences first; the first `false` releases the value and decodes to
    /// `DecodeError.Invalid`.
    Checked {
        layout: LayoutId,
        check: u32,
        value: CodecNodeId,
    },
}

/// A checked opaque type's check: `fn(value: T) -> bool` (`symbol`, a unit
/// function symbol, at `abi`), which borrows the value at the node's layout.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub struct CodecCheck {
    pub symbol: SymbolId,
    pub abi: FnAbiId,
}

impl CodecNode {
    pub fn layout(&self) -> LayoutId {
        match self {
            Self::Scalar(layout)
            | Self::Char(layout)
            | Self::Path(layout)
            | Self::Text { layout, .. }
            | Self::FlatText { layout, .. }
            | Self::Framed { layout, .. }
            | Self::Fields { layout, .. }
            | Self::Variant { layout, .. }
            | Self::Box { layout, .. }
            | Self::Sequence { layout, .. }
            | Self::Repeat { layout, .. }
            | Self::Map { layout, .. }
            | Self::Set { layout, .. }
            | Self::Checked { layout, .. } => *layout,
        }
    }

    /// Every child node, in encoding order.
    pub fn children(&self) -> Vec<CodecNodeId> {
        match self {
            Self::Scalar(_)
            | Self::Char(_)
            | Self::Path(_)
            | Self::Text { .. }
            | Self::FlatText { .. } => vec![],
            Self::Framed { value, .. } | Self::Checked { value, .. } => vec![*value],
            Self::Fields { fields, .. } => fields.clone(),
            Self::Variant { arms, .. } => arms.iter().flatten().copied().collect(),
            Self::Box { payload, .. } => vec![*payload],
            Self::Sequence { element, .. }
            | Self::Repeat { element, .. }
            | Self::Set { element, .. } => vec![*element],
            Self::Map { key, value, .. } => vec![*key, *value],
        }
    }
}

/// One `Encode` or `Decode` site's codec (`SideTables::codecs`).
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct CodecPlan {
    /// The concrete schema's SER-2 descriptor bytes.
    pub descriptor: Vec<u8>,
    pub nodes: Vec<CodecNode>,
    /// The encoded value's node.
    pub root: CodecNodeId,
    /// The `Bytes` node: an `Encode`'s result, a `Decode`'s operand.
    pub bytes: CodecNodeId,
    /// A `Decode`'s result: `Result[T, DecodeError]`, a `Variant` node whose
    /// arm 0 holds `root` and whose arm 1 holds a `Variant` of exactly
    /// `DECODE_ERRORS` fieldless arms. None for an `Encode`.
    pub result: Option<CodecNodeId>,
    /// The checks `Checked` nodes name (LIR schema revision 39); empty for a
    /// plan that reaches no checked type, and for every `Encode`.
    pub checks: Vec<CodecCheck>,
}

/// The `DecodeError` arms, in ordinal order (core-library.md).
pub const DECODE_ERRORS: [&str; 11] = [
    "Version",
    "SchemaMismatch",
    "Truncated",
    "Trailing",
    "Scalar",
    "Utf8",
    "Tag",
    "Length",
    "Duplicate",
    "Order",
    "Invalid",
];

/// `ENTRIES` and `INDEX`: the fields of a Map or Set container payload.
pub const CONTAINER_ENTRIES: u32 = 0;
pub const CONTAINER_INDEX: u32 = 1;

fn record(table: &LayoutTable, id: LayoutId) -> Result<&LayoutRecord, String> {
    table
        .layout(id)
        .ok_or_else(|| format!("layout {} outside the table", id.0))
}

/// The field of `ordinal`, which every field of a codec node's layout has once.
fn field(record: &LayoutRecord, ordinal: u32) -> Result<&FieldLayout, String> {
    let mut found = record.fields.iter().filter(|f| f.ordinal == ordinal);
    match (found.next(), found.next()) {
        (Some(f), None) => Ok(f),
        _ => Err(format!("no single field of ordinal {ordinal}")),
    }
}

fn fixed(record: &LayoutRecord) -> Result<u64, String> {
    match record.size {
        SizeSpec::Fixed(size) => Ok(size),
        SizeSpec::Dynamic(_) => Err("a dynamic extent".into()),
    }
}

/// The exact private flat field framing, independently of the producer's plan.
fn prefixed(r: &LayoutRecord) -> Result<(u64, LayoutId), String> {
    use crate::layout_types::SizeTerm;
    let expected = SizeSpec::Dynamic(vec![
        SizeTerm::Constant(8),
        SizeTerm::ReadLength {
            offset: 0,
            scalar: ScalarKind::U64,
        },
        SizeTerm::Add,
    ]);
    match (r.kind.clone(), &r.size, r.fields.as_slice()) {
        (LayoutKind::Flat, size, [field])
            if *size == expected && matches!(field.offset, 8 | 16) =>
        {
            Ok((field.offset, field.layout))
        }
        _ => Err("a codec node without flat length framing".into()),
    }
}

/// A check's `FnAbi`: a definition taking only the context and one borrowed
/// value at `layout`, returning a `bool` with no caller site or destination.
fn check_abi(table: &LayoutTable, abi: FnAbiId, layout: LayoutId) -> Result<(), String> {
    let a = table
        .fn_abis
        .get(abi.0 as usize)
        .ok_or("a check outside the FnAbi table")?;
    let borrowed = |p: &AbiParameter| {
        p.layout == layout
            && p.convention == Passing::Borrow
            && matches!(
                p.passing,
                ParameterPassing::ByValue | ParameterPassing::ReadOnlyAddress
            )
    };
    let boolean = a
        .result
        .as_ref()
        .and_then(|r| table.layout(r.layout))
        .is_some_and(|r| r.kind == LayoutKind::Scalar(ScalarKind::Bool));
    if a.shape != crate::abi_types::CallableShape::Definition
        || !a.context
        || a.environment
        || a.caller_site
        || a.destination
        || a.diverges
        || a.result_arena.is_some()
        || !boolean
        || !matches!(a.parameters.as_slice(), [p] if borrowed(p))
    {
        return Err("a check whose FnAbi is not fn(value: T) -> bool".into());
    }
    Ok(())
}

/// The payload of a block layout.
fn payload(table: &LayoutTable, id: LayoutId) -> Result<LayoutId, String> {
    match record(table, id)?.kind {
        LayoutKind::Block { payload } | LayoutKind::Box { payload } => Ok(payload),
        ref kind => Err(format!("{kind:?} is not a block")),
    }
}

/// The element of a block of a `Sequence` payload, and that payload.
fn sequence(table: &LayoutTable, id: LayoutId) -> Result<(LayoutId, LayoutId), String> {
    let payload = payload(table, id)?;
    match record(table, payload)?.kind {
        LayoutKind::Sequence { element }
        | LayoutKind::Sequential { element }
        | LayoutKind::SoaColumns { element } => Ok((payload, element)),
        ref kind => Err(format!("{kind:?} is not a sequence payload")),
    }
}

/// A core `Map` or `Set` (`name`) container payload: the payload, the
/// entries' sequence payload and element, and the index's sequence payload.
fn container(
    table: &LayoutTable,
    id: LayoutId,
    name: &str,
) -> Result<(LayoutId, LayoutId, LayoutId, LayoutId), String> {
    let payload = payload(table, id)?;
    let r = record(table, payload)?;
    let LayoutKind::Opaque { owner } = &r.kind else {
        return Err("a container payload that is not opaque".into());
    };
    if !owner.0.package.name_is("core")
        || !owner.0.modules.is_empty()
        || owner.0.name != name
        || r.fields.len() != if name == "Map" { 4 } else { 2 }
    {
        return Err(format!("a container payload that is not core {name}'s"));
    }
    let (entries, element) = sequence(table, field(r, CONTAINER_ENTRIES)?.layout)?;
    let (index, word) = sequence(table, field(r, CONTAINER_INDEX)?.layout)?;
    let scalar = if name == "Map" {
        ScalarKind::U64
    } else {
        ScalarKind::U32
    };
    if record(table, word)?.kind != LayoutKind::Scalar(scalar) {
        return Err("a container index with the wrong word layout".into());
    }
    if name == "Map" {
        if field(r, 2)?.layout != field(r, CONTAINER_INDEX)?.layout
            || record(table, field(r, 3)?.layout)?.kind != LayoutKind::Scalar(ScalarKind::U64)
        {
            return Err("a map without cached u64 hashes and a live count".into());
        }
        map_pair(table, element)?;
    }
    Ok((payload, entries, element, index))
}

/// Validate private optional storage independently of the library that emits it.
fn map_pair(table: &LayoutTable, entry: LayoutId) -> Result<&FieldLayout, String> {
    let r = record(table, entry)?;
    let tag = r.tag.as_ref().ok_or("a map entry without a tag")?;
    if r.kind != LayoutKind::Variant || tag.variants.len() != 2 {
        return Err("a map entry that is not optional".into());
    }
    let none = tag
        .variants
        .iter()
        .find(|v| v.ordinal == 0)
        .ok_or("map None")?;
    let some = tag
        .variants
        .iter()
        .find(|v| v.ordinal == 1)
        .ok_or("map Some")?;
    if !none.fields.is_empty() || some.fields.len() != 1 {
        return Err("a map entry that is not None/Some(pair)".into());
    }
    let field = &some.fields[0];
    let pair = record(table, field.layout)?;
    if pair.kind != LayoutKind::Aggregate || pair.fields.len() != 2 {
        return Err("a map entry payload that is not a pair".into());
    }
    Ok(field)
}

impl CodecPlan {
    pub fn node(&self, id: CodecNodeId) -> Result<&CodecNode, String> {
        self.nodes
            .get(id.index())
            .ok_or_else(|| format!("codec node {} outside the plan", id.0))
    }

    /// The plan agrees with `table`: each node's layout has the node's kind,
    /// and each child is at the layout its parent's layout gives it. A `Text`
    /// storage is the byte sequence of its slice; the decode result is
    /// `Result[root, DecodeError]`.
    pub fn check(&self, table: &LayoutTable) -> Result<(), String> {
        for id in [Some(self.root), Some(self.bytes), self.result]
            .into_iter()
            .flatten()
        {
            self.node(id)?;
        }
        if !matches!(self.node(self.bytes)?, CodecNode::Text { utf8: false, .. }) {
            return Err("the bytes node is not Bytes".into());
        }
        if let Some(result) = self.result {
            let CodecNode::Variant { arms, .. } = self.node(result)? else {
                return Err("a decode result that is not a variant".into());
            };
            let [ok, error] = arms.as_slice() else {
                return Err("a decode result without exactly two arms".into());
            };
            let (&[ok], &[error]) = (ok.as_slice(), error.as_slice()) else {
                return Err("a decode result arm without exactly one field".into());
            };
            if ok != self.root {
                return Err("a decode result whose Ok arm is not the root".into());
            }
            match self.node(error)? {
                CodecNode::Variant { arms, .. }
                    if arms.len() == DECODE_ERRORS.len() && arms.iter().all(Vec::is_empty) => {}
                _ => return Err("a decode error that is not DecodeError's variant".into()),
            }
        }
        let child = |id: CodecNodeId, layout: LayoutId| -> Result<(), String> {
            if self.node(id)?.layout() != layout {
                return Err(format!(
                    "codec node {} is not at its parent's layout {}",
                    id.0, layout.0
                ));
            }
            Ok(())
        };
        for node in &self.nodes {
            let r = record(table, node.layout())?;
            match node {
                CodecNode::Scalar(_) => {
                    if !matches!(r.kind, LayoutKind::Scalar(k) if k != ScalarKind::Pointer) {
                        return Err("a scalar node at a layout that is not a value scalar".into());
                    }
                }
                CodecNode::Char(_) => {
                    if r.kind != LayoutKind::Scalar(ScalarKind::U32) {
                        return Err("a char node at a layout that is not u32".into());
                    }
                }
                CodecNode::Path(_) => {
                    if r.kind != LayoutKind::Path {
                        return Err("a path node at a layout that is not a Path".into());
                    }
                }
                CodecNode::Text { storage, .. } => {
                    let LayoutKind::Slice { element } = r.kind else {
                        return Err("a text node at a layout that is not a slice".into());
                    };
                    if record(table, element)?.kind != LayoutKind::Scalar(ScalarKind::U8)
                        || record(table, *storage)?.kind != (LayoutKind::Sequence { element })
                    {
                        return Err("a text node that is not a byte slice of its storage".into());
                    }
                }
                CodecNode::FlatText { .. } => {
                    let (at, byte) = prefixed(r)?;
                    if at != 8 || record(table, byte)?.kind != LayoutKind::Scalar(ScalarKind::U8) {
                        return Err("a flat text node that is not prefixed bytes".into());
                    }
                }
                CodecNode::Framed { value, .. } => {
                    let (at, content) = prefixed(r)?;
                    if at != 8 || matches!(record(table, content)?.size, SizeSpec::Fixed(_)) {
                        return Err("a framed node without variable content".into());
                    }
                    child(*value, content)?;
                }
                CodecNode::Fields { fields, .. } => {
                    let flat = r.kind == LayoutKind::Flat;
                    if !matches!(r.kind, LayoutKind::Unit | LayoutKind::Aggregate) && !flat
                        || r.fields.len() != fields.len()
                    {
                        return Err("a fields node that does not cover its layout's fields".into());
                    }
                    for (ordinal, id) in fields.iter().enumerate() {
                        child(*id, field(r, ordinal as u32)?.layout)?;
                    }
                }
                CodecNode::Variant { arms, .. } => {
                    let tag = match (&r.kind, &r.tag) {
                        (LayoutKind::Variant | LayoutKind::Packed, Some(tag))
                            if matches!(tag.form, TagForm::Ordinal | TagForm::Niche { .. }) =>
                        {
                            tag
                        }
                        _ => return Err("a variant node at a layout without an ordinal".into()),
                    };
                    if tag.variants.len() != arms.len() {
                        return Err("a variant node that does not cover its arms".into());
                    }
                    for (ordinal, arm) in arms.iter().enumerate() {
                        let variant = tag
                            .variants
                            .iter()
                            .find(|v| v.ordinal == ordinal as u32)
                            .ok_or("a variant node arm without its variant")?;
                        if variant.fields.len() != arm.len() {
                            return Err("a variant arm that does not cover its fields".into());
                        }
                        for (i, id) in arm.iter().enumerate() {
                            let f = variant
                                .fields
                                .iter()
                                .find(|f| f.ordinal == i as u32)
                                .ok_or("a variant arm without its field")?;
                            child(*id, f.layout)?;
                        }
                    }
                }
                CodecNode::Box { payload: p, .. } => {
                    let payload = match r.kind {
                        LayoutKind::Box { payload } => {
                            fixed(record(table, payload)?)?;
                            payload
                        }
                        LayoutKind::Block { payload }
                            if record(table, payload)?.kind == LayoutKind::Flat =>
                        {
                            payload
                        }
                        _ => {
                            return Err(
                                "a box node at a layout that is not a box or flat block".into()
                            );
                        }
                    };
                    child(*p, payload)?;
                }
                CodecNode::Sequence { layout, element } => {
                    let e = if r.kind == LayoutKind::Flat {
                        let (at, e) = prefixed(r)?;
                        if at == 8 && fixed(record(table, e)?)? == 0 {
                            return Err("a flat sequence without an element count".into());
                        }
                        e
                    } else {
                        let (p, e) = sequence(table, *layout)?;
                        match record(table, p)?.kind {
                            LayoutKind::Sequence { .. } | LayoutKind::SoaColumns { .. } => {
                                fixed(record(table, e)?)?;
                            }
                            LayoutKind::Sequential { .. }
                                if record(table, e)?.kind == LayoutKind::Packed => {}
                            _ => return Err("a sequential codec without Packed elements".into()),
                        }
                        if matches!(record(table, p)?.kind, LayoutKind::SoaColumns { .. })
                            && !matches!(self.node(*element)?, CodecNode::Fields { .. })
                        {
                            return Err("a columns codec without record fields".into());
                        }
                        e
                    };
                    child(*element, e)?;
                }
                CodecNode::Repeat { element, .. } => {
                    let e = match r.kind {
                        LayoutKind::Inline { element, .. } | LayoutKind::Simd { element, .. } => {
                            element
                        }
                        _ => return Err("a repeat node at a layout that is not Inline/Simd".into()),
                    };
                    fixed(record(table, e)?)?;
                    child(*element, e)?;
                }
                CodecNode::Map { layout, key, value } => {
                    let (_, _, pair, _) = container(table, *layout, "Map")?;
                    let p = record(table, map_pair(table, pair)?.layout)?;
                    child(*key, field(p, 0)?.layout)?;
                    child(*value, field(p, 1)?.layout)?;
                }
                CodecNode::Set { layout, element } => {
                    let (_, _, e, _) = container(table, *layout, "Set")?;
                    child(*element, e)?;
                }
                CodecNode::Checked {
                    layout,
                    check,
                    value,
                } => {
                    if !matches!(
                        self.node(*value)?,
                        CodecNode::Fields { .. } | CodecNode::Variant { .. }
                    ) {
                        return Err(
                            "a checked node over a value that is not a record or variant".into(),
                        );
                    }
                    child(*value, *layout)?;
                    let c = self
                        .checks
                        .get(*check as usize)
                        .ok_or("a checked node without its check")?;
                    check_abi(table, c.abi, *layout)?;
                }
            }
        }
        if self.result.is_none() && !self.checks.is_empty() {
            return Err("an encode plan with checks".into());
        }
        // Each check borrows one layout: every node calling it is at that layout.
        let layouts = self.check_layouts();
        if layouts.contains(&None) {
            return Err("a codec check no checked node calls".into());
        }
        for node in &self.nodes {
            if let CodecNode::Checked { layout, check, .. } = node
                && layouts.get(*check as usize).copied().flatten() != Some(*layout)
            {
                return Err("a codec check called at two layouts".into());
            }
        }
        Ok(())
    }

    /// Per check, the layout of the CHECKED node calling it: what the check borrows.
    /// None for a check no node calls (which [`CodecPlan::check`] rejects).
    pub fn check_layouts(&self) -> Vec<Option<LayoutId>> {
        let mut out = vec![None; self.checks.len()];
        for node in &self.nodes {
            if let CodecNode::Checked { layout, check, .. } = node
                && let Some(slot) = out.get_mut(*check as usize)
            {
                *slot = Some(*layout);
            }
        }
        out
    }

    /// The layouts whose descriptor indices the program names: the blocks a
    /// decode allocates. Layouts the plan does not resolve are left out;
    /// `program` reports them.
    pub fn allocated(&self, table: &LayoutTable) -> Vec<LayoutId> {
        let mut out = vec![];
        // A decode with checks releases a rejected value through its root's descriptor.
        if !self.checks.is_empty()
            && let Ok(root) = self.node(self.root)
        {
            out.push(root.layout());
        }
        for node in &self.nodes {
            match node {
                CodecNode::Text { storage, .. } => out.push(*storage),
                CodecNode::Box { layout, .. } | CodecNode::Sequence { layout, .. } => {
                    out.extend(payload(table, *layout).ok())
                }
                CodecNode::Map { layout, .. } | CodecNode::Set { layout, .. } => {
                    let name = if matches!(node, CodecNode::Map { .. }) {
                        "Map"
                    } else {
                        "Set"
                    };
                    if let Ok((payload, entries, _, index)) = container(table, *layout, name) {
                        out.extend([payload, entries, index]);
                    }
                }
                CodecNode::Scalar(_)
                | CodecNode::Char(_)
                | CodecNode::Path(_)
                | CodecNode::FlatText { .. }
                | CodecNode::Framed { .. }
                | CodecNode::Fields { .. }
                | CodecNode::Variant { .. }
                | CodecNode::Repeat { .. }
                | CodecNode::Checked { .. } => {}
            }
        }
        out.sort_unstable();
        out.dedup();
        out
    }

    /// The runtime codec program of this plan (`hirsel_rt_v2.h`, "Codec
    /// programs"): `index` is the linked descriptor index of a layout. The
    /// plan must `check` against `table`.
    pub fn program(
        &self,
        table: &LayoutTable,
        index: &dyn Fn(LayoutId) -> Option<u32>,
    ) -> Result<Vec<u64>, String> {
        let descriptor = |id: LayoutId| {
            index(id)
                .map(u64::from)
                .ok_or_else(|| format!("layout {} has no descriptor index", id.0))
        };
        let top = self.result.unwrap_or(self.bytes);
        let mut words = vec![
            PROGRAM_VERSION,
            self.nodes.len() as u64,
            fixed(record(table, self.node(top)?.layout())?)?,
            u64::from(self.root.0),
            u64::from(top.0),
            u64::from(self.bytes.0),
            if self.checks.is_empty() {
                NO_NODE
            } else {
                descriptor(self.node(self.root)?.layout())?
            },
        ];
        let table_at = words.len();
        words.resize(table_at + self.nodes.len(), 0);
        let fields = |r: &LayoutRecord, ids: &[CodecNodeId], out: &mut Vec<u64>| {
            out.push(ids.len() as u64);
            for (ordinal, id) in ids.iter().enumerate() {
                out.push(field(r, ordinal as u32)?.offset);
                out.push(u64::from(id.0));
            }
            Ok::<_, String>(())
        };
        for (n, node) in self.nodes.iter().enumerate() {
            words[table_at + n] = words.len() as u64;
            let r = record(table, node.layout())?;
            let mut body = vec![];
            match node {
                CodecNode::Scalar(_) => {
                    let LayoutKind::Scalar(kind) = r.kind else {
                        return Err("a scalar node off its layout".into());
                    };
                    body.extend([NODE_SCALAR, scalar_code(kind)?]);
                }
                CodecNode::Char(_) => body.extend([NODE_SCALAR, SCALAR_CHAR]),
                CodecNode::Path(_) => body.push(NODE_PATH),
                CodecNode::Checked { check, value, .. } => {
                    body.extend([NODE_CHECKED, u64::from(*check), u64::from(value.0)])
                }
                CodecNode::Text { storage, utf8, .. } => {
                    body.extend([
                        NODE_TEXT,
                        u64::from(*utf8),
                        field(r, 0)?.offset,
                        field(r, 1)?.offset,
                        field(r, 2)?.offset,
                        descriptor(*storage)?,
                    ]);
                }
                CodecNode::FlatText { utf8, .. } => body.extend([NODE_FLAT_TEXT, u64::from(*utf8)]),
                CodecNode::Framed { value, .. } => body.extend([NODE_FRAMED, u64::from(value.0)]),
                CodecNode::Fields { fields: ids, .. } => {
                    if r.kind == LayoutKind::Flat {
                        body.extend([NODE_FLAT_FIELDS, ids.len() as u64]);
                        body.extend(ids.iter().map(|id| u64::from(id.0)));
                    } else {
                        body.push(NODE_FIELDS);
                        fields(r, ids, &mut body)?;
                    }
                }
                CodecNode::Variant { arms, .. } => {
                    if r.kind == LayoutKind::Packed && r.align == 1 {
                        body.extend([NODE_FLAT_VARIANT, arms.len() as u64]);
                        for arm in arms {
                            body.push(arm.len() as u64);
                            body.extend(arm.iter().map(|id| u64::from(id.0)));
                        }
                        words.extend(body);
                        continue;
                    }
                    let tag = r.tag.as_ref().ok_or("a variant node without a tag")?;
                    let (form, nullary) = match tag.form {
                        TagForm::Ordinal => (0, 0),
                        TagForm::Niche { nullary, .. } => (1, u64::from(nullary)),
                        TagForm::LiveBit { .. } => return Err("a live-bit variant".into()),
                    };
                    body.extend([NODE_VARIANT, form, tag.offset, nullary, arms.len() as u64]);
                    for (ordinal, arm) in arms.iter().enumerate() {
                        let variant = tag
                            .variants
                            .iter()
                            .find(|v| v.ordinal == ordinal as u32)
                            .ok_or("a variant arm without its variant")?;
                        body.push(arm.len() as u64);
                        for (i, id) in arm.iter().enumerate() {
                            let f = variant
                                .fields
                                .iter()
                                .find(|f| f.ordinal == i as u32)
                                .ok_or("a variant arm without its field")?;
                            body.extend([f.offset, u64::from(id.0)]);
                        }
                    }
                    for variant in &tag.variants {
                        let extent = if r.kind == LayoutKind::Packed {
                            let mut end = tag.offset + 4;
                            for f in &variant.fields {
                                let size = record(table, f.layout)?
                                    .size
                                    .storage()
                                    .ok_or("an unbounded Packed field")?;
                                end = end.max(
                                    f.offset.checked_add(size).ok_or("Packed extent overflow")?,
                                );
                            }
                            crate::layout::align_up(end, u64::from(r.align))
                                .map_err(|e| format!("{e:?}"))?
                        } else {
                            fixed(r)?
                        };
                        body.push(extent);
                    }
                }
                CodecNode::Box { payload: p, .. } => {
                    let payload = payload(table, node.layout())?;
                    let pr = record(table, payload)?;
                    body.extend([
                        NODE_BOX,
                        descriptor(payload)?,
                        match pr.size {
                            SizeSpec::Fixed(n) => n,
                            SizeSpec::Dynamic(_) => NO_NODE,
                        },
                        u64::from(pr.align),
                        u64::from(p.0),
                    ]);
                }
                CodecNode::Sequence { layout, element } => {
                    if r.kind == LayoutKind::Flat {
                        let (at, e) = prefixed(r)?;
                        let stride = if at == 8 {
                            fixed(record(table, e)?)?
                        } else {
                            0
                        };
                        body.extend([NODE_FLAT_SEQUENCE, at, stride, u64::from(element.0)]);
                    } else {
                        let (payload, e) = sequence(table, *layout)?;
                        let pr = record(table, payload)?;
                        let er = record(table, e)?;
                        match pr.kind {
                            LayoutKind::Sequence { .. } => {
                                body.push(NODE_SEQUENCE);
                                sequence_words(table, payload, e, descriptor(payload)?, &mut body)?;
                                body.push(u64::from(element.0));
                            }
                            LayoutKind::Sequential { .. } => body.extend([
                                NODE_SEQUENTIAL,
                                descriptor(payload)?,
                                u64::from(pr.align),
                                sequential_elements_offset(er).map_err(|e| format!("{e:?}"))?,
                                u64::from(element.0),
                            ]),
                            LayoutKind::SoaColumns { .. } => {
                                let CodecNode::Fields { fields: ids, .. } = self.node(*element)?
                                else {
                                    return Err("columns without record fields".into());
                                };
                                let quantum = er
                                    .fields
                                    .iter()
                                    .map(|f| record(table, f.layout).map(|r| u64::from(r.align)))
                                    .collect::<Result<Vec<_>, _>>()?
                                    .into_iter()
                                    .max()
                                    .unwrap_or(1);
                                body.extend([
                                    NODE_COLUMNS,
                                    descriptor(payload)?,
                                    u64::from(pr.align),
                                    soa_data_offset(quantum).map_err(|e| format!("{e:?}"))?,
                                    quantum,
                                    u64::from(element.0),
                                    ids.len() as u64,
                                ]);
                                let mut prefix = 0;
                                for (ordinal, id) in ids.iter().enumerate() {
                                    let stride =
                                        fixed(record(table, field(er, ordinal as u32)?.layout)?)?;
                                    body.extend([prefix, stride, u64::from(id.0)]);
                                    prefix += stride;
                                }
                            }
                            _ => return Err("a sequence program off its payload".into()),
                        }
                    }
                }
                CodecNode::Repeat { element, .. } => {
                    let (e, count) = match r.kind {
                        LayoutKind::Inline { element, count } => (element, count),
                        LayoutKind::Simd { element, lanes } => (element, u64::from(lanes)),
                        _ => return Err("a repeat node off its layout".into()),
                    };
                    body.extend([
                        NODE_REPEAT,
                        count,
                        fixed(record(table, e)?)?,
                        u64::from(element.0),
                    ]);
                }
                CodecNode::Map { layout, .. } | CodecNode::Set { layout, .. } => {
                    let map = matches!(node, CodecNode::Map { .. });
                    let name = if map { "Map" } else { "Set" };
                    let (payload, entries, element, index) = container(table, *layout, name)?;
                    let pr = record(table, payload)?;
                    body.extend([
                        if map { NODE_MAP } else { NODE_SET },
                        descriptor(payload)?,
                        fixed(pr)?,
                        u64::from(pr.align),
                        field(pr, CONTAINER_ENTRIES)?.offset,
                        field(pr, CONTAINER_INDEX)?.offset,
                    ]);
                    sequence_words(table, entries, element, descriptor(entries)?, &mut body)?;
                    let (_, word) = sequence(table, field(pr, CONTAINER_INDEX)?.layout)?;
                    body.extend([
                        descriptor(index)?,
                        u64::from(record(table, word)?.align),
                        fixed(record(table, word)?)?,
                    ]);
                    match node {
                        CodecNode::Map { key, value, .. } => {
                            let pair = map_pair(table, element)?;
                            let p = record(table, pair.layout)?;
                            body.extend([
                                u64::from(key.0),
                                pair.offset + field(p, 0)?.offset,
                                u64::from(value.0),
                                pair.offset + field(p, 1)?.offset,
                            ]);
                            let tag = record(table, element)?
                                .tag
                                .as_ref()
                                .ok_or("map entry tag")?;
                            let (form, bit) = match tag.form {
                                TagForm::Ordinal => (0, 0),
                                TagForm::LiveBit { bit } => (1, u64::from(bit)),
                                TagForm::Niche {
                                    nullary: 0,
                                    dataful: 1,
                                } => (2, 0),
                                _ => return Err("map entry tag ordinals".into()),
                            };
                            body.extend([
                                field(pr, 2)?.offset,
                                field(pr, 3)?.offset,
                                tag.offset,
                                form,
                                bit,
                            ]);
                        }
                        CodecNode::Set { element, .. } => {
                            body.extend([u64::from(element.0), 0, NO_NODE, 0])
                        }
                        _ => unreachable!(),
                    }
                }
            }
            words.extend(body);
        }
        Ok(words)
    }
}

/// A sequence payload's descriptor index, alignment, first element offset and
/// stride.
fn sequence_words(
    table: &LayoutTable,
    payload: LayoutId,
    element: LayoutId,
    index: u64,
    out: &mut Vec<u64>,
) -> Result<(), String> {
    let e = record(table, element)?;
    let elements = sequence_elements_offset(e).map_err(|e| format!("{e:?}"))?;
    if record(table, payload)?.kind != (LayoutKind::Sequence { element }) {
        return Err("a sequence program off its payload".into());
    }
    out.extend([index, u64::from(e.align), elements, fixed(e)?]);
    Ok(())
}

fn scalar_code(kind: ScalarKind) -> Result<u64, String> {
    use ScalarKind as S;
    Ok(match kind {
        S::Bool => 0,
        S::U8 => 1,
        S::U16 => 2,
        S::U32 => 3,
        S::U64 => 4,
        S::U128 => 5,
        S::I8 => 6,
        S::I16 => 7,
        S::I32 => 8,
        S::I64 => 9,
        S::I128 => 10,
        S::F32 => 11,
        S::F64 => 12,
        S::Pointer => return Err("a pointer is not a codec scalar".into()),
    })
}

/// The codec program's version and node kinds (`HIRSEL_RT_V2_CODEC_*`).
pub const PROGRAM_VERSION: u64 = 4;
pub const NODE_SCALAR: u64 = 0;
pub const NODE_TEXT: u64 = 1;
pub const NODE_FIELDS: u64 = 2;
pub const NODE_VARIANT: u64 = 3;
pub const NODE_BOX: u64 = 4;
pub const NODE_SEQUENCE: u64 = 5;
pub const NODE_REPEAT: u64 = 6;
pub const NODE_MAP: u64 = 7;
pub const NODE_SET: u64 = 8;
pub const NODE_FLAT_TEXT: u64 = 9;
pub const NODE_FRAMED: u64 = 10;
pub const NODE_FLAT_FIELDS: u64 = 11;
pub const NODE_FLAT_VARIANT: u64 = 12;
pub const NODE_FLAT_SEQUENCE: u64 = 13;
pub const NODE_SEQUENTIAL: u64 = 14;
pub const NODE_COLUMNS: u64 = 15;
pub const NODE_PATH: u64 = 16;
pub const NODE_CHECKED: u64 = 17;
/// A `NODE_SCALAR` code past the layout scalars: 4 bytes holding a Unicode
/// scalar value.
pub const SCALAR_CHAR: u64 = 13;
/// A Set's absent value node.
pub const NO_NODE: u64 = u64::MAX;

snapshot_enum!(CodecNode {
    Scalar(layout),
    Text {
        layout,
        storage,
        utf8
    },
    Fields { layout, fields },
    Variant { layout, arms },
    Box { layout, payload },
    Sequence { layout, element },
    Repeat { layout, element },
    Map { layout, key, value },
    Set { layout, element },
    FlatText { layout, utf8 },
    Framed { layout, value },
    Char(layout),
    Path(layout),
    Checked {
        layout,
        check,
        value
    },
});
snapshot_struct!(CodecCheck { symbol, abi });
snapshot_struct!(CodecPlan {
    descriptor,
    nodes,
    root,
    bytes,
    result,
    checks
});
