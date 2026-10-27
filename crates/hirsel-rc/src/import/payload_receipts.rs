use super::*;
use hirsel_lir::ir::{ConstructId, PayloadBindingId, ReceiptInput, SealDescriptorId};
use hirsel_lir::sealed_payload::*;

impl Importer<'_> {
    fn construction_receipt_source(&mut self, anchor: &SourceAnchor) -> Option<SourcePayload> {
        let source = self
            .instances
            .payload_sources(self.id)
            .iter()
            .find(|source| {
                &source.recipe.source == anchor
                    && matches!(source.recipe.operation, PayloadOperation::Allocate { .. })
            })?
            .clone();
        let descriptor = self
            .layouts
            .seal_descriptor(&self.program.operation_registry(), &source.root_type)
            .ok()?;
        descriptor
            .logical
            .facts
            .iter()
            .any(|fact| {
                matches!(
                    fact.class,
                    hirsel_lir::logical_storage::StorageClass::Block
                        | hirsel_lir::logical_storage::StorageClass::Bytes
                        | hirsel_lir::logical_storage::StorageClass::Shared
                )
            })
            .then_some(source)
    }

    pub(super) fn needs_construction_receipt(&mut self) -> bool {
        !self.instances.payload_sources(self.id).is_empty()
            && self
                .construction_receipt_source(&self.source_anchor_here())
                .is_some()
    }

    pub(super) fn construct_receipt(&mut self, value: ValueId, construct: ConstructId) -> R<()> {
        let site = &self.out.tables.constructs[construct.index()];
        let anchor = self.out.tables.source_anchors[site.anchor.index()].clone();
        let Some(source) = self.construction_receipt_source(&anchor) else {
            return Ok(());
        };
        let descriptor = self
            .layouts
            .seal_descriptor(&self.program.operation_registry(), &source.root_type)
            .map_err(|e| PassError::Layout(self.def.path.path().clone(), e))?;
        if descriptor.root_layout != self.layout_of(self.value_type(value)) {
            return malformed("original receipt construction root layout");
        }
        let site = &self.out.tables.constructs[construct.index()];
        let fields = site
            .fields
            .get(&self.out.pools.fields)
            .unwrap_or(&[])
            .to_vec();
        if fields.len() != source.recipe.inputs.len() {
            return malformed("original receipt construction operands");
        }
        let arena = match &source.recipe.operation {
            PayloadOperation::Allocate {
                destination: PayloadDestination::Arena(_),
            } => {
                let DestinationClass::Arena(slot) = site.destination else {
                    return unsupported("receipt Arena destination needs explicit slot custody");
                };
                let place = self.whole(slot);
                let address = self.address(&place)?;
                Some(self.operand_at(UseKind::Borrow, &place, address, None)?)
            }
            _ => None,
        };
        let mut loans = Vec::new();
        let mut inputs = Vec::new();
        for (field, original) in fields.iter().zip(&source.recipe.inputs) {
            let plain = self
                .layouts
                .seal_descriptor(&self.program.operation_registry(), &original.ty)
                .is_ok_and(|descriptor| {
                    matches!(
                        descriptor.logical.shape.nodes.as_slice(),
                        [hirsel_lir::structural::Shape::Leaf]
                    ) && descriptor.bits_root(self.layouts.table()).is_ok()
                });
            let receipt = if plain {
                None
            } else {
                let slot = self.receipt_slot(field.value).ok_or_else(|| {
                    PassError::Unsupported(format!(
                        "construction child needs paired original receipt custody: {:?}, {:?}",
                        original.ty, original.producer
                    ))
                })?;
                let metadata = self.whole(slot);
                let address = self.address(&metadata)?;
                let loan = self.synthetic_loan(address);
                let receipt = self.value(metadata.ty, Holding::Borrowed(loan));
                let reaching = self.reaching(slot)?;
                self.push(Inst::Read {
                    result: receipt,
                    address,
                    reaching,
                    mode: ReadMode::Borrow,
                });
                loans.push(loan);
                Some(receipt)
            };
            inputs.push(ReceiptInput {
                original: field.value,
                receipt,
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
            arena: arena.into(),
            result: receipt,
            value,
            shape,
            binding,
            inputs,
        });
        for loan in loans {
            self.state.loans[loan.index()] = Tri::No;
            self.push(Inst::EndLoan {
                loan,
                if_active: false,
            });
        }
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
