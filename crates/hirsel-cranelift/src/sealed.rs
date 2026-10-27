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
        let (status, output) = self.prepare_seal(result, grant, root, shape, origin)?;
        // Prepare reports exhaustion without recording a language failure.
        // Attribute it to this grant before entering ordinary failure cleanup.
        let grant = self.get1(grant)?;
        let exhausted = self.exhausted_block(grant, origin, failure)?;
        self.branch_to(status, exhausted)?;
        let owner = self.load_ptr(output);
        self.define1(result, owner)
    }

    /// Use the original entry companion for heap-bearing roots. Pointer-free
    /// roots need only a temporary bits snapshot. Neither path reconstructs
    /// source provenance from addresses or from the release program.
    pub(crate) fn prepare_seal(
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
        descriptor
            .check(self.em.layouts())
            .map_err(|what| crate::CodegenError::Invalid {
                definition: self.definition().clone(),
                what,
            })?;
        let bits = descriptor.bits_root(self.em.layouts()).is_ok();
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
        let grant = self.get1(grant)?;
        let image = self.stack(layout)?;
        let physical = self.layout_shape(layout)?;
        self.store(physical, value, image)?;
        let index = self.descriptor_value(layout)?;
        let receipt = if let Some(slot) = companion {
            let place = self.slots[slot.index()].ok_or_else(|| crate::CodegenError::Invalid {
                definition: self.definition().clone(),
                what: "seal receipt entry slot is unbound".into(),
            })?;
            self.load_ptr(place.ptr)
        } else {
            self.rt1(RuntimeEntry::ReceiptBits, &[self.context, index, image.ptr])?
        };
        let output = self.out_word();
        let status = self.rt1(
            RuntimeEntry::SealPrepareCommit,
            &[self.context, grant, image.ptr, index, receipt, site, output],
        )?;
        // Release only the temporary bits holder. The entry slot keeps its
        // metadata custody through success and failure cleanup.
        if bits {
            self.rt(RuntimeEntry::ReceiptRelease, &[receipt])?;
        }
        Ok((status, output))
    }

    pub(crate) fn seal_receipt_slot(
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
