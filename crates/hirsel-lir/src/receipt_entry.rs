//! Private entry-slot bindings for the demand-driven receipt ABI. These are
//! local claims, not original-source authority or executable custody proofs.
use crate::abi_types::FnAbi;
use crate::ir::{Holding, LirUnit, Ref, SlotId, SlotKind, ValueId, ValueRepr};
use crate::layout_types::{LayoutKind, LayoutTable};
use hirsel_codec::snapshot_struct;
use hirsel_syntax::ContentHash;
use std::collections::BTreeSet;

/// Original parameter or capture ordinal and its initial metadata-slot version.
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct ReceiptEntrySlot {
    pub source: u32,
    pub slot: SlotId,
    pub version: ValueId,
}

#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct ReceiptEntry {
    pub contract: ContentHash,
    pub parameters: Vec<ReceiptEntrySlot>,
    pub captures: Vec<ReceiptEntrySlot>,
    /// Alias of the caller's empty output slot. Publication is normal-only;
    /// rejected admission and entered failure publish no result receipt.
    pub result: Option<SlotId>,
}

impl ReceiptEntry {
    pub fn versions(&self) -> impl Iterator<Item = ValueId> + '_ {
        self.parameters
            .iter()
            .chain(&self.captures)
            .map(|p| p.version)
    }

    pub fn remap_values(&mut self, map: &impl Fn(ValueId) -> ValueId) {
        for binding in self.parameters.iter_mut().chain(&mut self.captures) {
            binding.version = map(binding.version);
        }
    }

    pub(crate) fn visit(&self, visit: &mut impl FnMut(Ref)) {
        for binding in self.parameters.iter().chain(&self.captures) {
            visit(Ref::Slot(binding.slot));
            visit(Ref::Value(binding.version));
        }
        if let Some(result) = self.result {
            visit(Ref::Slot(result));
        }
    }

    /// Check namespace, coverage, local types and version-only entry names.
    /// Full source type/profile pairing, environment offsets, aliasing and
    /// lifetime/edge publication remain independent executable obligations.
    pub fn check<S>(
        &self,
        abi: &FnAbi,
        unit: &LirUnit<S>,
        layouts: &LayoutTable,
    ) -> Result<(), &'static str> {
        crate::abi::check_receipt_contract(abi, layouts)?;
        let contract = abi
            .receipts
            .as_ref()
            .ok_or("receipt entry has no callee contract")?;
        if self.contract != contract.content()? {
            return Err("receipt entry names a different complete contract");
        }
        if self.parameters.len() != contract.parameters.len()
            || self.captures.len() != contract.captures.len()
            || self.result.is_some() != contract.result.is_some()
        {
            return Err("receipt entry coverage differs from its contract");
        }
        let mut versions = BTreeSet::new();
        for (binding, parameter) in self.parameters.iter().zip(&contract.parameters) {
            if binding.source != parameter.source {
                return Err("receipt parameter entry selectors are not canonical");
            }
            let physical = abi
                .parameters
                .iter()
                .position(|p| p.source == binding.source)
                .ok_or("receipt entry has no original declared parameter")?;
            if unit
                .parameters
                .get(physical)
                .and_then(|v| unit.values.get(v.index()))
                .is_none()
            {
                return Err("receipt entry has no original parameter value");
            }
            self.input(
                binding,
                SlotKind::ReceiptParameter(binding.source),
                unit,
                layouts,
                &mut versions,
            )?;
        }
        for (binding, capture) in self.captures.iter().zip(&contract.captures) {
            if binding.source != capture.ordinal {
                return Err("receipt capture entry selectors are not canonical");
            }
            if unit
                .captures
                .get(binding.source as usize)
                .and_then(|v| unit.values.get(v.index()))
                .is_none()
            {
                return Err("receipt entry has no original capture value");
            }
            self.input(
                binding,
                SlotKind::ReceiptCapture(binding.source),
                unit,
                layouts,
                &mut versions,
            )?;
        }
        if let Some(result) = self.result {
            Self::slot(result, SlotKind::ReceiptResult, unit, layouts)?;
        }
        Ok(())
    }

    fn slot<S>(
        slot: SlotId,
        kind: SlotKind,
        unit: &LirUnit<S>,
        layouts: &LayoutTable,
    ) -> Result<crate::ir::LirTypeId, &'static str> {
        let record = unit
            .slots
            .get(slot.index())
            .ok_or("receipt entry names a foreign slot")?;
        if record.kind != kind {
            return Err("receipt entry slot is in the wrong namespace");
        }
        let ty = unit
            .types
            .get(record.ty.index())
            .ok_or("receipt entry slot has a foreign type")?;
        if ty.repr != ValueRepr::PayloadReceipt
            || !layouts
                .layout(ty.layout)
                .is_some_and(|l| matches!(l.kind, LayoutKind::PayloadReceipt { .. }))
        {
            return Err("receipt entry slot lacks private metadata representation");
        }
        Ok(record.ty)
    }

    fn input<S>(
        &self,
        binding: &ReceiptEntrySlot,
        kind: SlotKind,
        unit: &LirUnit<S>,
        layouts: &LayoutTable,
        versions: &mut BTreeSet<ValueId>,
    ) -> Result<(), &'static str> {
        let ty = Self::slot(binding.slot, kind, unit, layouts)?;
        let version = unit
            .values
            .get(binding.version.index())
            .ok_or("receipt entry names a foreign version")?;
        if version.ty != ty {
            return Err("receipt entry version and slot types differ");
        }
        // The slot has custody according to the contract. Its initial version
        // names those bytes; it is not an additional owning SSA holder.
        if version.holding != Holding::Plain {
            return Err("receipt entry slot version is not a plain version name");
        }
        if !versions.insert(binding.version) {
            return Err("receipt entry defines one version twice");
        }
        Ok(())
    }
}

snapshot_struct!(ReceiptEntrySlot {
    source,
    slot,
    version
});
snapshot_struct!(ReceiptEntry {
    contract,
    parameters,
    captures,
    result
});
