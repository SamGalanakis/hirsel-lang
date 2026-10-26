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

impl<'m, 'a> FnEmitter<'m, 'a> {
    pub(super) fn seal_arena(
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
        let owner = self.builder.load(self.module.ptr_type(), output, 8);
        self.define(result, owner)
    }

    /// The temporary receipt describes the original inline bits, not a heap
    /// graph inferred from a scan. Nonempty/unsupported scopes still refuse in
    /// the checked runtime; no inventory is synthesized by this emitter.
    pub(super) fn prepare_seal_bits(
        &mut self,
        result: ValueId,
        grant: ValueId,
        root: ValueId,
        shape: SealDescriptorId,
        origin: SourceAnchorId,
    ) -> Emitted<(Value<'m>, Value<'m>)> {
        let Some(descriptor) = self.unit.tables.seal_descriptors.get(shape.index()) else {
            return self.invalid("seal descriptor is missing");
        };
        let layout = match descriptor.bits_root(self.inputs.layouts) {
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
        let grant = self.get(grant)?;
        let image = self.stack(layout)?;
        self.store(self.layout_shape(layout)?, value, image);
        let index = self.descriptor(layout)?;
        let bits = self.runtime(RuntimeEntry::ReceiptBits)?;
        let receipt = self.builder.call(bits, &[self.context, index, image.ptr]);
        let output = self.prologue.alloca(self.module.ptr_type(), 8);
        let prepare = self.runtime(RuntimeEntry::SealPrepareCommit)?;
        let status = self.builder.call(
            prepare,
            &[self.context, grant, image.ptr, index, receipt, site, output],
        );
        // Every returned status releases the private snapshot before branching.
        // This never releases the caller's root or grant on refusal.
        let release = self.runtime(RuntimeEntry::ReceiptRelease)?;
        self.builder.call(release, &[receipt]);
        Ok((status, output))
    }
}
