//! Physical receipt entry bindings. C still requires independent source/custody
//! authority; binding a claimed entry does not grant executable admission.
use super::*;
use hirsel_lir::sealed_payload_source::ReceiptMode;

impl<'m, 'a> FnEmitter<'m, 'a> {
    pub(super) fn bind_receipt_entry(&mut self) -> Emitted {
        let unit = self.unit;
        let abi = self.signature.abi;
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
            .check_transport(abi, unit, self.inputs.layouts)
            .map_err(|what| crate::CodegenError::Invalid {
                definition: self.definition().clone(),
                what: what.into(),
            })?;
        if let Some(environment_layout) = entry.environment {
            let (_, fields) = hirsel_lir::receipt_closure::transport_environment(
                self.inputs.layouts,
                environment_layout,
                abi.receipts.as_ref().unwrap(),
            )
            .map_err(|what| crate::CodegenError::Invalid {
                definition: self.definition().clone(),
                what: what.into(),
            })?;
            let environment = self
                .environment
                .ok_or_else(|| crate::CodegenError::Invalid {
                    definition: self.definition().clone(),
                    what: "receipt entry has no environment pointer".into(),
                })?;
            let payload = self.payload_of(environment, environment_layout)?;
            for (binding, field) in entry.captures.iter().zip(fields) {
                let mut place =
                    self.field_place(payload, field.offset, field.layout, field.unaligned)?;
                place.layout = self.lir_type(unit.slots[binding.slot.index()].ty)?.layout;
                self.slots[binding.slot.index()] = Some(place);
            }
        }
        let first = self.signature.hidden.len()
            + abi.parameters.iter().map(|p| p.slots.len()).sum::<usize>();
        let positions = hirsel_lir::abi::receipt_slots(abi, first).map_err(|what| {
            crate::CodegenError::Invalid {
                definition: self.definition().clone(),
                what: what.into(),
            }
        })?;
        for (binding, position) in entry.parameters.iter().zip(&positions.parameters) {
            let layout = self.lir_type(unit.slots[binding.slot.index()].ty)?.layout;
            let pointer = self.module.parameter(self.function, position.index as u32);
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
                self.store(Shape::Scalar(ScalarKind::Pointer), pointer, place);
                place
            };
            self.slots[binding.slot.index()] = Some(place);
        }
        if let (Some(slot), Some(index)) = (entry.result, positions.result) {
            let layout = self.lir_type(unit.slots[slot.index()].ty)?.layout;
            self.slots[slot.index()] = Some(Place {
                columns: None,
                ptr: self.module.parameter(self.function, index as u32),
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
    pub(super) fn check_receipt_call(
        &self,
        site: &CallSite,
        abi: &hirsel_lir::abi_types::FnAbi,
    ) -> Emitted {
        hirsel_lir::abi::check_receipt_contract(abi, self.inputs.layouts)
            .and_then(|()| {
                hirsel_lir::receipt_transport::check_call_contract(abi, site, &self.unit.pools)
            })
            .map_err(|what| crate::CodegenError::Invalid {
                definition: self.definition().clone(),
                what: what.into(),
            })?;
        if abi.receipts.is_some()
            && !matches!(
                site.callee,
                Callee::Direct(_) | Callee::Bound { .. } | Callee::Indirect(_)
            )
        {
            return self.unsupported("receipt call has no callable entry");
        }
        Ok(())
    }

    /// Append metadata holders/addresses in the contract's physical order. This
    /// acquires no holder and writes neither caller mut storage nor the output.
    pub(super) fn receipt_call_arguments(
        &mut self,
        site: &CallSite,
        arguments: &mut Vec<Value<'m>>,
    ) -> Emitted<Option<(ValueId, Place<'m>)>> {
        let Some(receipts) = &site.receipts else {
            return Ok(None);
        };
        for argument in &receipts.arguments {
            let pointer = match *argument {
                ReceiptArgument::Borrow { receipt, .. } | ReceiptArgument::Take { receipt, .. } => {
                    if !self.is_receipt_value(receipt)? {
                        return self.invalid("a receipt call passed a language value as metadata");
                    }
                    self.get(receipt)?
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
mod tests;

#[cfg(test)]
mod call_tests;

#[cfg(test)]
mod sealed_tests;

#[cfg(test)]
mod sealed_drop_tests;

#[cfg(test)]
pub(crate) mod seal_bits_tests;

#[cfg(test)]
mod guard_tests;

mod construct;

#[cfg(test)]
mod construct_tests;

#[cfg(test)]
mod capture_tests;
