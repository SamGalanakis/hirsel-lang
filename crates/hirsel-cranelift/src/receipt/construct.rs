use crate::function::{FnEmitter, V};
use crate::module::Emitted;
use hirsel_lir::ir::*;
use hirsel_lir::layout_types::LayoutKind;
use hirsel_lir::runtime_abi::RuntimeEntry;

impl FnEmitter<'_, '_, '_> {
    pub(crate) fn payload_receipt(
        &mut self,
        result: ValueId,
        value: ValueId,
        shape: SealDescriptorId,
        binding: PayloadBindingId,
        operands: (PoolRange<ReceiptInput>, Option<ValueId>),
    ) -> Emitted {
        let (inputs, arena) = operands;
        use hirsel_lir::sealed_payload::*;
        let Some((descriptor, binding)) = self.unit.tables.payload_binding(binding) else {
            return self.invalid("missing original construction recipe");
        };
        let inputs = self.pool(inputs, &self.unit.pools.receipt_inputs)?.to_vec();
        binding
            .check_receipt_inputs(&inputs)
            .map_err(|what| crate::CodegenError::Invalid {
                definition: self.definition().clone(),
                what: what.into(),
            })?;
        if descriptor != shape
            || !matches!(binding.recipe.operation, PayloadOperation::Allocate { .. })
        {
            return self.unsupported("receipt construction requires an original heap recipe");
        }
        let destination = matches!(
            (&binding.recipe.operation, arena),
            (
                PayloadOperation::Allocate {
                    destination: PayloadDestination::Heap
                },
                None
            ) | (
                PayloadOperation::Allocate {
                    destination: PayloadDestination::Arena(_)
                },
                Some(_)
            )
        );
        if !destination
            || arena.is_some_and(|arena| {
                self.unit.types[self.unit.values[arena.index()].ty.index()].repr != ValueRepr::Arena
            })
        {
            return self.invalid("receipt construction has another original destination");
        }
        let descriptor = &self.unit.tables.seal_descriptors[shape.index()];
        if self.value_type(value)?.layout != descriptor.root_layout
            || !matches!(&self.layout(self.value_type(result)?.layout)?.kind,
                LayoutKind::PayloadReceipt { root } if root == &descriptor.root_type)
        {
            return self.invalid("construction receipt describes another root");
        }
        let kind = match self.layout(descriptor.root_layout)?.kind {
            LayoutKind::Block { .. } | LayoutKind::Box { .. } => 1,
            _ => 0,
        };
        let sources = binding.recipe.inputs.clone();
        let mut children = Vec::new();
        let mut temporary = Vec::new();
        for (input, source) in inputs.iter().zip(&sources) {
            if let Some(receipt) = input.receipt {
                if !self.is_receipt_value(receipt)? {
                    return self.invalid("construction child lacks a receipt layout");
                }
                children.push(self.get1(receipt)?);
            } else if matches!(
                source.producer,
                PayloadProducer::Literal(PayloadLiteral::String(_) | PayloadLiteral::Bytes(_))
            ) {
                let receipt = self.construction_snapshot(input.original, 2, &[])?;
                children.push(receipt);
                temporary.push(receipt);
            } else {
                // C proves omitted inputs have a pointer-free source type.
                // A literal with storage must take the explicit static path.
                let layout = self.layout(self.value_type(input.original)?.layout)?;
                if !layout.scan.is_empty()
                    || matches!(
                        layout.kind,
                        LayoutKind::Block { .. }
                            | LayoutKind::Box { .. }
                            | LayoutKind::Slice { .. }
                            | LayoutKind::Shared { .. }
                    )
                {
                    return self.unsupported("construction input needs its original receipt");
                }
            }
        }
        let receipt = self.construction_snapshot_in(value, kind, &children, arena)?;
        for child in temporary {
            self.rt(RuntimeEntry::ReceiptRelease, &[child])?;
        }
        self.define(result, V::One(receipt))
    }

    pub(crate) fn arena_inventory(
        &mut self,
        arena: ValueId,
        allocation: SourceAnchorId,
        value: ValueId,
        shape: SealDescriptorId,
        companions: (Option<ValueId>, Option<ValueId>),
    ) -> Emitted {
        let (previous, receipt) = companions;
        use hirsel_lir::sealed_payload::*;
        if previous.is_some() {
            return self.unsupported("arena replacement requires original holder correspondence");
        }
        let descriptor = &self.unit.tables.seal_descriptors[shape.index()];
        descriptor
            .check(self.em.layouts())
            .map_err(|what| crate::CodegenError::Invalid {
                definition: self.definition().clone(),
                what,
            })?;
        let layout = descriptor.root_layout;
        if let Some(receipt) = receipt
            && !matches!(&self.layout(self.value_type(receipt)?.layout)?.kind,
                LayoutKind::PayloadReceipt { root } if root == &descriptor.root_type)
        {
            return self.invalid("inventory receipt has another original root type");
        }
        let payload = match self.layout(layout)?.kind {
            LayoutKind::Block { payload } | LayoutKind::Box { payload } => payload,
            _ => return self.unsupported("arena inventory requires an original block"),
        };
        // Complete original source facts plus typed physical coverage. An empty
        // release program alone would miss arena-local and Shared child edges.
        if receipt.is_none()
            && (!self.layout(payload)?.scan.is_empty()
                || !self.layout(payload)?.drop.is_empty()
                || descriptor.logical.facts.iter().skip(1).any(|fact| {
                    !matches!(
                        fact.class,
                        hirsel_lir::logical_storage::StorageClass::Inline
                    )
                }))
        {
            return self.unsupported("arena inventory payload needs paired original receipts");
        }
        let source = &self.unit.tables.source_anchors[allocation.index()];
        let bindings: Vec<_> = self
            .unit
            .tables
            .seal_payloads
            .iter()
            .filter(|table| table.descriptor == shape)
            .flat_map(|table| &table.bindings)
            .filter(|binding| &binding.recipe.source == source)
            .collect();
        if !matches!(bindings.as_slice(), [binding] if matches!(binding.recipe.operation,
            PayloadOperation::Allocate { destination: PayloadDestination::Arena(_) }))
            || self.value_type(arena)?.repr != ValueRepr::Arena
            || self.layout(self.value_type(arena)?.layout)?.kind != LayoutKind::Arena
            || self.value_type(value)?.layout != layout
        {
            return self.invalid("arena inventory differs from its original allocation recipe");
        }
        let image = self.stack(layout)?;
        let root = self.get(value)?;
        let shape = self.layout_shape(layout)?;
        self.store(shape, root, image)?;
        let arena = self.get1(arena)?;
        let index = self.descriptor_value(layout)?;
        let mut args = vec![self.context, arena, index, image.ptr];
        let entry = if let Some(receipt) = receipt {
            args.push(self.get1(receipt)?);
            RuntimeEntry::ReceiptArenaInventoryPaired
        } else {
            RuntimeEntry::ReceiptArenaInventory
        };
        self.rt(entry, &args)?;
        Ok(())
    }

    fn construction_snapshot(
        &mut self,
        value: ValueId,
        kind: u8,
        children: &[cranelift_codegen::ir::Value],
    ) -> Emitted<cranelift_codegen::ir::Value> {
        self.construction_snapshot_in(value, kind, children, None)
    }

    fn construction_snapshot_in(
        &mut self,
        value: ValueId,
        kind: u8,
        children: &[cranelift_codegen::ir::Value],
        arena: Option<ValueId>,
    ) -> Emitted<cranelift_codegen::ir::Value> {
        use cranelift_codegen::ir::{InstBuilder, types};
        let layout = self.value_type(value)?.layout;
        let image = self.stack(layout)?;
        let size = self.fixed_size(layout)?;
        self.zero(image.ptr, size, image.align);
        let bits = self.get(value)?;
        let shape = self.layout_shape(layout)?;
        self.store(shape, bits, image)?;
        let (array, _) = self.stack_bytes(children.len().max(1) as u64 * 8, 8);
        for (i, &child) in children.iter().enumerate() {
            self.store_at(child, array, (i * 8) as u64);
        }
        let index = self.descriptor_value(layout)?;
        let kind = self.b.ins().iconst(types::I8, i64::from(kind));
        let count = self.u64c(children.len() as u64);
        let (entry, args) = if let Some(arena) = arena {
            let arena = self.get1(arena)?;
            (
                RuntimeEntry::ReceiptArenaConstruct,
                vec![self.context, arena, index, image.ptr, array, count],
            )
        } else {
            (
                RuntimeEntry::ReceiptConstruct,
                vec![self.context, index, image.ptr, kind, array, count],
            )
        };
        self.rt1(entry, &args)
    }
}
