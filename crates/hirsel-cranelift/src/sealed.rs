//! Physical reads under C's independently checked exclusive owner loan.
use crate::function::{FnEmitter, Place};
use crate::module::Emitted;
use hirsel_lir::ir::{FailureId, SealDescriptorId, SourceAnchorId, ValueId, ValueRepr};
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

impl FnEmitter<'_, '_, '_> {
    pub(crate) fn seal_arena(
        &mut self,
        result: ValueId,
        grant: ValueId,
        root: ValueId,
        shape: SealDescriptorId,
        failure: FailureId,
        origin: SourceAnchorId,
    ) -> Emitted {
        let (status, output) = self.prepare_seal_bits(result, grant, root, shape, origin)?;
        self.status_guard(status, failure)?;
        let owner = self.load_ptr(output);
        self.define1(result, owner)
    }

    /// Original pointer-free bytes only. The checked runtime independently
    /// rejects retained inventory, physical backing and ordinary region guards.
    pub(crate) fn prepare_seal_bits(
        &mut self,
        result: ValueId,
        grant: ValueId,
        root: ValueId,
        shape: SealDescriptorId,
        origin: SourceAnchorId,
    ) -> Emitted<(cranelift_codegen::ir::Value, cranelift_codegen::ir::Value)> {
        let Some(descriptor) = self.unit.tables.seal_descriptors.get(shape.index()) else {
            return self.invalid("seal descriptor is missing");
        };
        let layout = match descriptor.bits_root(self.em.layouts()) {
            Ok(layout) => layout,
            Err(error) => return self.unsupported(error),
        };
        if self.value_type(root)?.layout != layout
            || self.value_type(grant)?.repr != ValueRepr::Arena
            || self.layout(self.value_type(grant)?.layout)?.kind != LayoutKind::Arena
            || self.value_type(result)?.repr != ValueRepr::Handle
            || !matches!(self.layout(self.value_type(result)?.layout)?.kind,
                LayoutKind::SealedRegion { root } if root == layout)
        {
            return self.invalid("seal operands disagree with their original bits descriptor");
        }
        let code = self.anchor_code(origin)?;
        let (_, site) = self.failure_sites(code);
        let value = self.get(root)?;
        let grant = self.get1(grant)?;
        let image = self.stack(layout)?;
        let physical = self.layout_shape(layout)?;
        self.store(physical, value, image)?;
        let index = self.descriptor_value(layout)?;
        let receipt = self.rt1(RuntimeEntry::ReceiptBits, &[self.context, index, image.ptr])?;
        let output = self.out_word();
        let status = self.rt1(
            RuntimeEntry::SealPrepareCommit,
            &[self.context, grant, image.ptr, index, receipt, site, output],
        )?;
        // Release the private snapshot on both statuses before failure cleanup.
        self.rt(RuntimeEntry::ReceiptRelease, &[receipt])?;
        Ok((status, output))
    }
}
