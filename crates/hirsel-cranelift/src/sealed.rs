//! Physical reads under C's independently checked exclusive owner loan.
use crate::function::{FnEmitter, Place};
use crate::module::Emitted;
use hirsel_lir::ir::{ValueId, ValueRepr};
use hirsel_lir::layout_types::LayoutKind;
use hirsel_lir::runtime_abi::RuntimeEntry;

impl FnEmitter<'_, '_, '_> {
    pub(crate) fn borrow_sealed_root(&mut self, result: ValueId, owner: ValueId) -> Emitted {
        let layout = self.value_type(result)?.layout;
        if self.value_type(owner)?.repr != ValueRepr::Handle
            || !matches!(self.layout(self.value_type(owner)?.layout)?.kind,
                LayoutKind::SealedRegion { root } if root == layout)
        {
            return self.invalid("sealed root read disagrees with owner layout");
        }
        let shape = self.layout_shape(layout)?;
        let owner = self.get1(owner)?;
        let ptr = self.rt1(RuntimeEntry::SealedRoot, &[self.context, owner])?;
        // Unit still calls the helper: no representation can bypass the
        // runtime's run/liveness check on the opaque native owner identity.
        let place = Place {
            columns: None,
            ptr,
            layout,
            align: self.layout(layout)?.align,
            block: None,
        };
        let value = self.load(shape, place);
        self.define(result, value)
    }
}
