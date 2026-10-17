//! Concrete operation schema and independent declaration interface.
use crate::abi_types::{CallMultiplicity, FnAbi};
use crate::layout::LayoutError;
use hirsel_codec::snapshot_enum;
use hirsel_core_contract::OperationId;
use hirsel_syntax::DefPath;
use std::hash::Hash;

#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub enum OperationArgument {
    Type(NativeType),
    Size(u64),
}

/// The same erased axes as LayoutBuilder's type identity. Callable values need
/// only multiplicity: their invocation signature does not affect their layout.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum NativeType {
    Primitive(Primitive),
    Tuple(Vec<NativeType>),
    Named(DefPath, Vec<OperationArgument>),
    Record(Vec<(bool, NativeType)>),
    Variant(Vec<Vec<NativeType>>),
    Function(CallMultiplicity),
    Box(Box<NativeType>),
    Builtin(BuiltinType, Vec<OperationArgument>),
}

impl std::hash::Hash for NativeType {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        hirsel_codec::to_text(self).hash(state);
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum Primitive {
    Bool,
    U8,
    U16,
    U32,
    U64,
    U128,
    I8,
    I16,
    I32,
    I64,
    I128,
    F32,
    F64,
    String,
    Bytes,
    Never,
}
snapshot_enum!(Primitive {
    Bool,
    U8,
    U16,
    U32,
    U64,
    U128,
    I8,
    I16,
    I32,
    I64,
    I128,
    F32,
    F64,
    String,
    Bytes,
    Never
});

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum BuiltinType {
    Array,
    List,
    Map,
    Set,
    SortedMap,
    Inline,
    Simd,
    Packed,
    Shared,
    Slots,
    SlotId,
    Region,
    Iter,
    Task,
    Sender,
    Receiver,
    Mutex,
    Atomic,
    Ptr,
    FnPtr,
    Shape,
}
snapshot_enum!(BuiltinType {
    Array,
    List,
    Map,
    Set,
    SortedMap,
    Inline,
    Simd,
    Packed,
    Shared,
    Slots,
    SlotId,
    Region,
    Iter,
    Task,
    Sender,
    Receiver,
    Mutex,
    Atomic,
    Ptr,
    FnPtr,
    Shape
});
/// Independently supplied operation declarations. Implementations derive from
/// the declaration and actuals, never from the candidate unit or its call ABI.
pub trait OperationDeclarations {
    fn derive(
        &self,
        operation: &OperationId,
        arguments: &[OperationArgument],
        target: &crate::TargetSpec,
    ) -> Result<(FnAbi, crate::LayoutTable), LayoutError>;
}
/// The empty declaration set for units without operation calls.
impl OperationDeclarations for () {
    fn derive(
        &self,
        _: &OperationId,
        _: &[OperationArgument],
        _: &crate::TargetSpec,
    ) -> Result<(FnAbi, crate::LayoutTable), LayoutError> {
        Err(LayoutError::Invalid("missing operation declaration".into()))
    }
}

/// Compare the complete ABI, including logical type identity, with table-local
/// layout indices compared as rooted graphs. Recursive block layouts terminate
/// through the visited pair set; arena numbers have no cross-table meaning.
pub(crate) fn matches_abi(
    expected: &FnAbi,
    expected_table: &crate::LayoutTable,
    actual: &FnAbi,
    actual_table: &crate::LayoutTable,
) -> bool {
    let mut expected = expected.clone();
    let mut pairs = Vec::new();
    for (e, a) in expected.parameters.iter_mut().zip(&actual.parameters) {
        pairs.push((e.layout, a.layout));
        e.layout = a.layout;
    }
    if let (Some(e), Some(a)) = (&mut expected.result, &actual.result) {
        pairs.push((e.layout, a.layout));
        e.layout = a.layout;
    }
    if expected != *actual {
        return false;
    }
    let mut seen = std::collections::BTreeSet::new();
    while let Some((e, a)) = pairs.pop() {
        if !seen.insert((e, a)) {
            continue;
        }
        let (Some(e), Some(a)) = (expected_table.layout(e), actual_table.layout(a)) else {
            return false;
        };
        let mut e = e.clone();
        let mut a = a.clone();
        // As in Layouts::same_scalar, an internal scalar and the same typed
        // primitive are interchangeable. Their allocation keys differ; all
        // physical facts still agree, and FnAbiKey binds logical type identity.
        if matches!(e.kind, crate::layout_types::LayoutKind::Scalar(_)) && e.kind == a.kind {
            e.key.instance = None;
            a.key.instance = None;
        }
        let mut e_refs = Vec::new();
        let mut a_refs = Vec::new();
        erase_layout_refs(&mut e, &mut e_refs);
        erase_layout_refs(&mut a, &mut a_refs);
        if e != a || e_refs.len() != a_refs.len() {
            return false;
        }
        pairs.extend(e_refs.into_iter().zip(a_refs));
    }
    true
}

fn erase_layout_refs(record: &mut crate::LayoutRecord, refs: &mut Vec<crate::LayoutId>) {
    use crate::layout_types::*;
    fn take(id: &mut LayoutId, refs: &mut Vec<LayoutId>) {
        refs.push(*id);
        *id = LayoutId(0);
    }
    fn columns(c: &mut SoaColumns, refs: &mut Vec<LayoutId>) {
        for c in &mut c.columns {
            take(&mut c.layout, refs);
        }
    }
    fn scan(steps: &mut [ScanStep], refs: &mut Vec<LayoutId>) {
        for step in steps {
            match step {
                ScanStep::Inline { layout, .. } => take(layout, refs),
                ScanStep::Sequence { element, .. } | ScanStep::Repeat { element, .. } => {
                    take(element, refs)
                }
                ScanStep::ActiveVariant { variants, .. } => {
                    for (_, steps) in variants {
                        scan(steps, refs);
                    }
                }
                ScanStep::Columns(c) => columns(c, refs),
                ScanStep::Counted { .. }
                | ScanStep::Uncounted { .. }
                | ScanStep::SliceOwner { .. }
                | ScanStep::Environment { .. }
                | ScanStep::Handle { .. } => {}
            }
        }
    }
    fn drop(steps: &mut [DropStep], refs: &mut Vec<LayoutId>) {
        for step in steps {
            match step {
                DropStep::Child { layout, .. } | DropStep::Thaw { layout, .. } => {
                    take(layout, refs)
                }
                DropStep::ElementsReverse { element, .. }
                | DropStep::RepeatReverse { element, .. } => take(element, refs),
                DropStep::ActiveVariant { variants, .. } => {
                    for (_, steps) in variants {
                        drop(steps, refs);
                    }
                }
                DropStep::ColumnsReverse(c) => columns(c, refs),
                DropStep::Finalize
                | DropStep::Release { .. }
                | DropStep::Environment { .. }
                | DropStep::CloseHandle { .. } => {}
            }
        }
    }
    match &mut record.kind {
        LayoutKind::Inline { element, .. }
        | LayoutKind::Simd { element, .. }
        | LayoutKind::Slice { element }
        | LayoutKind::Iterator { element }
        | LayoutKind::SoaColumns { element }
        | LayoutKind::Sequence { element }
        | LayoutKind::Sequential { element } => take(element, refs),
        LayoutKind::Block { payload }
        | LayoutKind::Box { payload }
        | LayoutKind::Shared { payload } => take(payload, refs),
        LayoutKind::Frozen { value } => take(value, refs),
        LayoutKind::Callable {
            environment: Some(e),
        } => take(e, refs),
        LayoutKind::Unit
        | LayoutKind::Scalar(_)
        | LayoutKind::Aggregate
        | LayoutKind::Environment { .. }
        | LayoutKind::Variant
        | LayoutKind::Packed
        | LayoutKind::Opaque { .. }
        | LayoutKind::Handle
        | LayoutKind::Path
        | LayoutKind::Arena
        | LayoutKind::Callable { environment: None }
        | LayoutKind::Resource
        | LayoutKind::Header
        | LayoutKind::Flat => {}
    }
    for field in &mut record.fields {
        take(&mut field.layout, refs);
    }
    if let Some(tag) = &mut record.tag {
        for v in &mut tag.variants {
            for field in &mut v.fields {
                take(&mut field.layout, refs);
            }
        }
    }
    scan(&mut record.scan, refs);
    drop(&mut record.drop, refs);
}

snapshot_enum!(OperationArgument { Type(ty), Size(n) });
snapshot_enum!(NativeType { Primitive(p), Tuple(items), Named(path, args), Record(fields), Variant(variants), Function(multiplicity), Box(inner), Builtin(kind, args) });
