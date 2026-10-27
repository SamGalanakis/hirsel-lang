use super::super::*;

impl<'m, 'a> FnEmitter<'m, 'a> {
    pub(in crate::emit) fn payload_receipt(
        &mut self,
        result: ValueId,
        value: ValueId,
        shape: SealDescriptorId,
        binding: PayloadBindingId,
        inputs: PoolRange<ReceiptInput>,
    ) -> Emitted {
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
            || !matches!(
                binding.recipe.operation,
                PayloadOperation::Allocate {
                    destination: PayloadDestination::Heap
                }
            )
        {
            return self.unsupported("receipt construction requires an original heap recipe");
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
                children.push(self.get(receipt)?);
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
        let receipt = self.construction_snapshot(value, kind, &children)?;
        for child in temporary {
            let release = self.runtime(RuntimeEntry::ReceiptRelease)?;
            self.builder.call(release, &[child]);
        }
        self.define(result, receipt)
    }

    pub(in crate::emit) fn arena_inventory(
        &mut self,
        arena: ValueId,
        allocation: SourceAnchorId,
        value: ValueId,
        shape: SealDescriptorId,
        previous: Option<ValueId>,
    ) -> Emitted {
        use hirsel_lir::sealed_payload::*;
        if previous.is_some() {
            return self.unsupported("arena replacement requires original holder correspondence");
        }
        let descriptor = &self.unit.tables.seal_descriptors[shape.index()];
        descriptor
            .check(self.inputs.layouts)
            .map_err(|what| crate::CodegenError::Invalid {
                definition: self.definition().clone(),
                what,
            })?;
        let layout = descriptor.root_layout;
        let payload = match self.layout(layout)?.kind {
            LayoutKind::Block { payload } | LayoutKind::Box { payload } => payload,
            _ => return self.unsupported("arena inventory requires an original block"),
        };
        // Complete original source facts plus typed physical coverage. An empty
        // release program alone would miss arena-local and Shared child edges.
        if !self.layout(payload)?.scan.is_empty()
            || !self.layout(payload)?.drop.is_empty()
            || descriptor.logical.facts.iter().skip(1).any(|fact| {
                !matches!(
                    fact.class,
                    hirsel_lir::logical_storage::StorageClass::Inline
                )
            })
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
        self.store(self.layout_shape(layout)?, root, image);
        let arena = self.get(arena)?;
        let index = self.descriptor(layout)?;
        let publish = self.runtime(RuntimeEntry::ReceiptArenaInventory)?;
        self.builder
            .call(publish, &[self.context, arena, index, image.ptr]);
        Ok(())
    }

    fn construction_snapshot(
        &mut self,
        value: ValueId,
        kind: u8,
        children: &[Value<'m>],
    ) -> Emitted<Value<'m>> {
        let layout = self.value_type(value)?.layout;
        let image = self.stack(layout)?;
        self.builder
            .memset(image.ptr, 0, self.fixed_size(layout)?, image.align);
        let bits = self.get(value)?;
        self.store(self.layout_shape(layout)?, bits, image);
        let array = self.prologue.alloca(
            self.module
                .array_type(self.module.ptr_type(), children.len().max(1) as u64),
            8,
        );
        for (i, &child) in children.iter().enumerate() {
            let at = self.builder.offset(
                array,
                self.module
                    .const_int(self.module.int_type(64), (i * 8) as u128),
            );
            self.builder.store(child, at, 8);
        }
        let index = self.descriptor(layout)?;
        let kind = self
            .module
            .const_int(self.module.int_type(8), u128::from(kind));
        let count = self
            .module
            .const_int(self.module.int_type(64), children.len() as u128);
        let snapshot = self.runtime(RuntimeEntry::ReceiptConstruct)?;
        Ok(self.builder.call(
            snapshot,
            &[self.context, index, image.ptr, kind, array, count],
        ))
    }
}
