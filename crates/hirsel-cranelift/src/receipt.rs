//! Physical receipt entry bindings, without source/custody admission.
use crate::function::{FnEmitter, Place, Shape, V};
use crate::module::Emitted;
use hirsel_lir::ir::{CallSite, Callee, ReceiptArgument, SlotKind, ValueId};
use hirsel_lir::layout_types::{LayoutKind, ScalarKind};
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
    pub(crate) fn check_receipt_call(
        &self,
        site: &CallSite,
        abi: &hirsel_lir::abi_types::FnAbi,
    ) -> Emitted {
        hirsel_lir::abi::check_receipt_contract(abi, self.em.layouts())
            .and_then(|()| {
                hirsel_lir::receipt_transport::check_call_contract(abi, site, &self.unit.pools)
            })
            .map_err(|what| crate::CodegenError::Invalid {
                definition: self.definition().clone(),
                what: what.into(),
            })?;
        if let Some(contract) = &abi.receipts {
            // A source-selected capture-free callback may require the Arena's
            // existing inventory carrier without appending any metadata words.
            let ordinary_bound = matches!(site.callee, Callee::Bound { .. })
                && contract.parameters.is_empty()
                && contract.result.is_none()
                && contract.captures.is_empty()
                && contract.inventory.iter().all(|grant| {
                    matches!(
                        grant,
                        hirsel_lir::sealed_payload_source::ReceiptGrant::Parameter(_)
                    )
                });
            if !ordinary_bound
                && (!matches!(site.callee, Callee::Direct(_))
                    || abi.environment
                    || !contract.captures.is_empty())
            {
                return self
                    .unsupported("receipt calls require a direct entry without capture transport");
            }
        }
        Ok(())
    }

    /// Append metadata holders/addresses in the contract's physical order. This
    /// acquires no holder and writes neither caller mut storage nor the output.
    pub(crate) fn receipt_call_arguments(
        &mut self,
        site: &CallSite,
        arguments: &mut Vec<cranelift_codegen::ir::Value>,
    ) -> Emitted<Option<(ValueId, Place)>> {
        let Some(receipts) = &site.receipts else {
            return Ok(None);
        };
        for argument in &receipts.arguments {
            let pointer = match *argument {
                ReceiptArgument::Borrow { receipt, .. } | ReceiptArgument::Take { receipt, .. } => {
                    if !self.is_receipt_value(receipt)? {
                        return self.invalid("a receipt call passed a language value as metadata");
                    }
                    self.get1(receipt)?
                }
                ReceiptArgument::Mut { address, .. } => {
                    let place = self.place(address)?;
                    let record = self.layout(place.layout)?;
                    if !matches!(record.kind, LayoutKind::PayloadReceipt { .. })
                        || place.columns.is_some()
                        || place.align < record.align
                    {
                        return self
                            .invalid("a receipt mut carrier is not an aligned metadata slot");
                    }
                    place.ptr
                }
            };
            arguments.push(pointer);
        }
        let Some(result) = receipts.result else {
            return Ok(None);
        };
        if !self.is_receipt_value(result)? {
            return self.invalid("a receipt result lacks metadata representation");
        }
        let place = self.stack(self.value_type(result)?.layout)?;
        arguments.push(place.ptr);
        Ok(Some((result, place)))
    }
}

#[cfg(test)]
pub(crate) mod tests;

#[cfg(test)]
mod sealed_tests;

#[cfg(test)]
pub(crate) mod seal_bits_tests;

mod construct;

#[cfg(test)]
mod construct_tests;
