//! Descriptor-bound LIR bindings for independently reconstructed source recipes.
use crate::ir::{ReceiptInput, SealDescriptorId, ValueId};
pub use crate::sealed_payload_source::*;
use hirsel_codec::snapshot_struct;

/// Concrete original operands captured before physical construction. The journal
/// transition is committed only on construction success. Input values must be
/// ordinary operands at the source site's actual construction/transition.
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct PayloadBinding {
    pub recipe: PayloadRecipe,
    pub values: Vec<ValueId>,
}

/// The table hangs off the existing descriptor identity. It does not modify or
/// reinterpret ArenaInventory or any existing serialized field.
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct PayloadTable {
    pub descriptor: SealDescriptorId,
    pub bindings: Vec<PayloadBinding>,
}

snapshot_struct!(PayloadBinding { recipe, values });
snapshot_struct!(PayloadTable {
    descriptor,
    bindings
});

impl PayloadBinding {
    /// Local ordered operand pairing, not source authority. C separately checks
    /// the complete recipe against original-source observations.
    pub fn check_receipt_inputs(&self, inputs: &[ReceiptInput]) -> Result<(), &'static str> {
        if inputs.len() != self.values.len()
            || inputs.len() != self.recipe.inputs.len()
            || !inputs
                .iter()
                .zip(&self.values)
                .all(|(input, original)| input.original == *original)
        {
            return Err("payload receipt inputs differ from ordered original binding");
        }
        Ok(())
    }
}

#[cfg(test)]
mod receipt_tests;
