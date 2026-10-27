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
        let (status, output) = self.prepare_seal(result, grant, root, shape, origin)?;
        // Prepare reports exhaustion without recording a language failure.
        // Attribute it to this grant before entering ordinary failure cleanup.
        let zero = self.module.const_int(self.module.int_type(8), 0);
        let failed = self.builder.icmp(IntPredicate::Ne, status, zero);
        let exhausted = self.exhausted_block(self.get(grant)?, origin, failure)?;
        let next = self.module.append_block(self.function);
        self.builder.cold_cond_br(failed, exhausted, next);
        self.builder.position(next);
        let owner = self.builder.load(self.module.ptr_type(), output, 8);
        self.define(result, owner)
    }

    /// Use the original entry companion for heap-bearing roots. Pointer-free
    /// roots need only a temporary bits snapshot. Neither path reconstructs
    /// source provenance from addresses or from the release program.
    pub(super) fn prepare_seal(
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
        descriptor
            .check(self.inputs.layouts)
            .map_err(|what| crate::CodegenError::Invalid {
                definition: self.definition().clone(),
                what,
            })?;
        let bits = descriptor.bits_root(self.inputs.layouts).is_ok();
        let layout = descriptor.root_layout;
        let companion = if bits {
            None
        } else {
            Some(self.seal_receipt_slot(root, &descriptor.root_type)?)
        };
        if self.value_type(root)?.layout != layout
            || self.value_type(grant)?.repr != ValueRepr::Arena
            || self.layout(self.value_type(grant)?.layout)?.kind != LayoutKind::Arena
            || self.value_type(result)?.repr != ValueRepr::Handle
            || !matches!(self.layout(self.value_type(result)?.layout)?.kind,
                LayoutKind::SealedRegion { root } if root == layout)
        {
            return self.invalid("seal operands disagree with their original descriptor");
        }
        let code = self.anchor_code(origin)?;
        let (_, site) = self.failure_sites(code);
        let value = self.get(root)?;
        let grant = self.get(grant)?;
        let image = self.stack(layout)?;
        self.store(self.layout_shape(layout)?, value, image);
        let index = self.descriptor(layout)?;
        let receipt = if let Some(slot) = companion {
            let place = self.slots[slot.index()].ok_or_else(|| crate::CodegenError::Invalid {
                definition: self.definition().clone(),
                what: "seal receipt entry slot is unbound".into(),
            })?;
            self.load(Shape::Scalar(ScalarKind::Pointer), place)
        } else {
            let snapshot = self.runtime(RuntimeEntry::ReceiptBits)?;
            self.builder
                .call(snapshot, &[self.context, index, image.ptr])
        };
        let output = self.prologue.alloca(self.module.ptr_type(), 8);
        let prepare = self.runtime(RuntimeEntry::SealPrepareCommit)?;
        let status = self.builder.call(
            prepare,
            &[self.context, grant, image.ptr, index, receipt, site, output],
        );
        // Only the bits path acquired a temporary metadata holder. An entry
        // companion remains in its slot for the ordinary cleanup on either edge.
        if bits {
            let release = self.runtime(RuntimeEntry::ReceiptRelease)?;
            self.builder.call(release, &[receipt]);
        }
        Ok((status, output))
    }

    pub(super) fn seal_receipt_slot(
        &self,
        mut root: ValueId,
        root_type: &hirsel_lir::operation::NativeType,
    ) -> Emitted<hirsel_lir::ir::SlotId> {
        use hirsel_lir::ir::{AddressBase, Inst};
        for _ in 0..=self.unit.values.len() {
            if let Some(entry) = &self.unit.tables.receipt_entry
                && let Some(binding) = entry.parameters.iter().find(|binding| {
                    self.unit.parameters.get(binding.source as usize) == Some(&root)
                })
            {
                let slot = &self.unit.slots[binding.slot.index()];
                let layout = self.lir_type(slot.ty)?.layout;
                if !matches!(&self.layout(layout)?.kind,
                    LayoutKind::PayloadReceipt { root } if root == root_type)
                {
                    return self.invalid("seal receipt entry describes another root type");
                }
                return Ok(binding.slot);
            }
            let parent = self.unit.instructions.iter().find_map(|inst| match *inst {
                Inst::CopyValue { result, value } | Inst::Weaken { result, value }
                    if result == root =>
                {
                    Some(value)
                }
                Inst::Read {
                    result,
                    reaching,
                    address,
                    ..
                } if result == root
                    && self.unit.addresses[address.index()].projection.len == 0
                    && !matches!(
                        self.unit.addresses[address.index()].base,
                        AddressBase::Address(_)
                    ) =>
                {
                    Some(reaching)
                }
                Inst::Store {
                    version,
                    value,
                    address,
                }
                | Inst::Replace {
                    version,
                    value,
                    address,
                    ..
                } if version == root
                    && self.unit.addresses[address.index()].projection.len == 0
                    && !matches!(
                        self.unit.addresses[address.index()].base,
                        AddressBase::Address(_)
                    ) =>
                {
                    Some(value)
                }
                _ => None,
            });
            match parent {
                Some(value) => root = value,
                None => break,
            }
        }
        self.unsupported("heap seal root requires its paired original receipt entry")
    }
}
