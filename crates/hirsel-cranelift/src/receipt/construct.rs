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
        let receipt = self.construction_snapshot(value, kind, &children)?;
        for child in temporary {
            self.rt(RuntimeEntry::ReceiptRelease, &[child])?;
        }
        self.define(result, V::One(receipt))
    }

    fn construction_snapshot(
        &mut self,
        value: ValueId,
        kind: u8,
        children: &[cranelift_codegen::ir::Value],
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
        self.rt1(
            RuntimeEntry::ReceiptConstruct,
            &[self.context, index, image.ptr, kind, array, count],
        )
    }
}
