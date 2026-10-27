use super::*;
use hirsel_lir::ir::{ConstructId, PayloadBindingId, ReceiptInput, SealDescriptorId};
use hirsel_lir::sealed_payload::*;

impl Importer<'_> {
    fn construction_receipt_source(&self, anchor: &SourceAnchor) -> Option<&SourcePayload> {
        self.instances
            .payload_sources(self.id)
            .iter()
            .find(|source| {
                &source.recipe.source == anchor
                    && matches!(
                        source.recipe.operation,
                        PayloadOperation::Allocate {
                            destination: PayloadDestination::Heap
                        }
                    )
                    && matches!(
                        source.root_type,
                        hirsel_lir::operation::NativeType::Builtin(
                            hirsel_lir::operation::BuiltinType::List,
                            _
                        )
                    )
            })
    }

    pub(super) fn needs_construction_receipt(&self) -> bool {
        !self.instances.payload_sources(self.id).is_empty()
            && self
                .construction_receipt_source(&self.source_anchor_here())
                .is_some()
    }

    pub(super) fn construct_receipt(&mut self, value: ValueId, construct: ConstructId) -> R<()> {
        let site = &self.out.tables.constructs[construct.index()];
        let anchor = &self.out.tables.source_anchors[site.anchor.index()];
        let Some(source) = self.construction_receipt_source(anchor).cloned() else {
            return Ok(());
        };
        let descriptor = self
            .layouts
            .seal_descriptor(&self.program.operation_registry(), &source.root_type)
            .map_err(|e| PassError::Layout(self.def.path.path().clone(), e))?;
        if descriptor.root_layout != self.layout_of(self.value_type(value)) {
            return malformed("original receipt construction root layout");
        }
        let fields = site
            .fields
            .get(&self.out.pools.fields)
            .unwrap_or(&[])
            .to_vec();
        if fields.len() != source.recipe.inputs.len() {
            return malformed("original receipt construction operands");
        }
        let mut inputs = Vec::new();
        for (field, original) in fields.iter().zip(&source.recipe.inputs) {
            if !matches!(
                original.ty,
                hirsel_lir::operation::NativeType::Primitive(hirsel_lir::operation::Primitive::U64)
            ) {
                return unsupported(format!(
                    "construction child needs paired original receipt custody: {:?}, {:?}",
                    original.ty, original.producer
                ));
            }
            inputs.push(ReceiptInput {
                original: field.value,
                receipt: None,
            });
        }
        let shape = SealDescriptorId(self.out.tables.seal_descriptors.len() as u32);
        self.out.tables.seal_descriptors.push(descriptor);
        let binding = PayloadBindingId(
            self.out
                .tables
                .seal_payloads
                .iter()
                .map(|table| table.bindings.len() as u32)
                .sum(),
        );
        self.out.tables.seal_payloads.push(PayloadTable {
            descriptor: shape,
            bindings: vec![PayloadBinding {
                recipe: source.recipe,
                values: fields.iter().map(|f| f.value).collect(),
            }],
        });
        let layout = self.layouts.payload_receipt(&source.root_type);
        let ty = self.intern(layout);
        let receipt = self.fresh(ty);
        let inputs = PoolRange::push(&mut self.out.pools.receipt_inputs, inputs);
        self.push(Inst::PayloadReceipt {
            result: receipt,
            value,
            shape,
            binding,
            inputs,
        });
        self.failure.pending.push(receipt);
        let slot = self.new_slot(ty, SlotKind::Local, ScopeId(0));
        self.push(Inst::StorageLive { slot });
        self.state.live(slot);
        let metadata = self.whole(slot);
        let address = self.address(&metadata)?;
        self.store(&metadata, address, receipt)?;
        self.receipt_values.insert(value, slot);
        Ok(())
    }
}
