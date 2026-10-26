//! Physical reads under C's independently checked exclusive owner loan.
use super::*;

impl<'m, 'a> FnEmitter<'m, 'a> {
    pub(super) fn borrow_sealed_root(&mut self, result: ValueId, owner: ValueId) -> Emitted {
        let layout = self.value_type(result)?.layout;
        if self.value_type(owner)?.repr != ValueRepr::Handle
            || !matches!(self.layout(self.value_type(owner)?.layout)?.kind,
                LayoutKind::SealedRegion { root } if root == layout)
        {
            return self.invalid("sealed root read disagrees with owner layout");
        }
        let shape = self.layout_shape(layout)?;
        let root = self.runtime(RuntimeEntry::SealedRoot)?;
        // Even Unit must check owner liveness and run identity. The result is
        // a call-bounded view of root bytes, never the opaque owner identity.
        let ptr = self.builder.call(root, &[self.context, self.get(owner)?]);
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
