//! The ambient counted heap is a real Arena value, produced without allocation.
use super::{build::Builder, *};
pub(super) fn signatures(types: &Arc<TypeInterner>) -> Vec<Signature> {
    let mut b = Builder::new(types, "");
    let mut f = b.function("", "Arena uses {}", true);
    let mut result = b.types.get(f.result).unwrap().clone();
    result.allocation_target = Some(AllocationTarget::Heap);
    result.modes = Modes::global();
    result.modes.sendability = Sendability::Sendable;
    result.drop_allocation = DropAllocation::NoAlloc;
    f.result = b.types.intern(result);
    let mut signature = b.signature("$intrinsic.heap", SignatureKind::Function(f), false);
    signature.metadata.visibility = hirsel_syntax::Visibility::Private;
    vec![signature]
}
