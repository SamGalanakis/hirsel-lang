//! Physical receipt entry bindings, without source/custody admission.
use crate::function::{FnEmitter, Place, Shape, V};
use crate::module::Emitted;
use hirsel_lir::ir::SlotKind;
use hirsel_lir::layout_types::ScalarKind;
use hirsel_lir::sealed_payload_source::ReceiptMode;

impl FnEmitter<'_, '_, '_> {
    pub(crate) fn bind_receipt_entry(&mut self) -> Emitted {
        let unit = self.unit;
        let abi = self.sig.abi;
        let Some(entry) = &unit.tables.receipt_entry else {
            if abi.receipts.is_some()
                || abi.key.receipt_contract.is_some()
                || unit.slots.iter().any(|s| {
                    matches!(
                        s.kind,
                        SlotKind::ReceiptParameter(_)
                            | SlotKind::ReceiptCapture(_)
                            | SlotKind::ReceiptResult
                    )
                })
            {
                return self.invalid("receipt entry bindings are missing");
            }
            return Ok(());
        };
        entry
            .check(abi, unit, self.em.layouts())
            .map_err(|what| crate::CodegenError::Invalid {
                definition: self.definition().clone(),
                what: what.into(),
            })?;
        if !entry.captures.is_empty() {
            return self
                .unsupported("receipt capture fields require a physical environment contract");
        }
        let first =
            self.sig.hidden.len() + abi.parameters.iter().map(|p| p.slots.len()).sum::<usize>();
        let positions = hirsel_lir::abi::receipt_slots(abi, first).map_err(|what| {
            crate::CodegenError::Invalid {
                definition: self.definition().clone(),
                what: what.into(),
            }
        })?;
        for (binding, position) in entry.parameters.iter().zip(&positions.parameters) {
            let layout = self.lir_type(unit.slots[binding.slot.index()].ty)?.layout;
            let pointer = self.entry_params[position.index];
            let place = if position.mode == ReceiptMode::MutSlot {
                Place {
                    columns: None,
                    ptr: pointer,
                    layout,
                    align: self.layout(layout)?.align,
                    block: None,
                }
            } else {
                let place = self.stack(layout)?;
                self.store(Shape::Scalar(ScalarKind::Pointer), V::One(pointer), place)?;
                place
            };
            self.slots[binding.slot.index()] = Some(place);
        }
        if let (Some(slot), Some(index)) = (entry.result, positions.result) {
            let layout = self.lir_type(unit.slots[slot.index()].ty)?.layout;
            self.slots[slot.index()] = Some(Place {
                columns: None,
                ptr: self.entry_params[index],
                layout,
                align: self.layout(layout)?.align,
                block: None,
            });
        }
        // Every private slot must be covered exactly once by the checked entry.
        for (index, slot) in unit.slots.iter().enumerate() {
            if matches!(
                slot.kind,
                SlotKind::ReceiptParameter(_)
                    | SlotKind::ReceiptCapture(_)
                    | SlotKind::ReceiptResult
            ) && self.slots[index].is_none()
            {
                return self.invalid("receipt entry leaves a private slot unbound");
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
