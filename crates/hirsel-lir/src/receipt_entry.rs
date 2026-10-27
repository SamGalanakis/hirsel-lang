//! Private entry-slot bindings for the demand-driven receipt ABI. These are
//! local claims, not original-source authority or executable custody proofs.
use crate::abi_types::FnAbi;
use crate::ir::{Holding, LirUnit, Ref, SlotId, SlotKind, ValueId, ValueRepr};
use crate::layout_types::{LayoutId, LayoutKind, LayoutTable};
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

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReceiptEntry {
    pub contract: ContentHash,
    pub parameters: Vec<ReceiptEntrySlot>,
    pub captures: Vec<ReceiptEntrySlot>,
    /// Alias of the caller's empty output slot. Publication is normal-only;
    /// rejected admission and entered failure publish no result receipt.
    pub result: Option<SlotId>,
    /// Exact physical wrapper when capture companions are present.
    pub environment: Option<LayoutId>,
}

impl std::hash::Hash for ReceiptEntry {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.contract.hash(state);
        self.parameters.hash(state);
        self.captures.hash(state);
        self.result.hash(state);
        if let Some(environment) = self.environment {
            "hirsel.receipt-entry-environment.v1".hash(state);
            environment.hash(state);
        }
    }
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
        if let Some(environment) = self.environment {
            visit(Ref::Layout(environment));
        }
        for binding in self.parameters.iter().chain(&self.captures) {
            visit(Ref::Slot(binding.slot));
            visit(Ref::Value(binding.version));
        }
        if let Some(result) = self.result {
            visit(Ref::Slot(result));
        }
    }

    /// Check namespace, coverage, local types and version-only entry names.
    /// Full source type/profile pairing, aliasing and lifetime/edge
    /// publication remain independent executable obligations.
    pub fn check<S>(
        &self,
        abi: &FnAbi,
        unit: &LirUnit<S>,
        layouts: &LayoutTable,
    ) -> Result<(), &'static str> {
        self.check_inner(abi, unit, layouts, None, false)
    }

    /// Structural entry bindings for a complete owning-capture schema. This
    /// supplies neither source authority nor executable capture pairing.
    pub fn check_with_source<S>(
        &self,
        abi: &FnAbi,
        unit: &LirUnit<S>,
        layouts: &LayoutTable,
        source: &crate::sealed_payload_source::ReceiptSource,
    ) -> Result<(), &'static str> {
        source.check_structure()?;
        if unit.header.definition != source.original.definition
            || unit.header.instance != source.instance
            || unit.header.implementation_revision != source.implementation_revision
        {
            return Err("receipt entry differs from its selected original source");
        }
        if abi.receipts.as_ref() != Some(&source.contract) {
            return Err("receipt entry source differs from its complete code contract");
        }
        if !source.contract.captures.is_empty() && self.environment.is_none() {
            return Err("receipt capture entry lacks its complete environment");
        }
        self.check_inner(abi, unit, layouts, Some(source), false)
    }

    /// Physical emitter checks after admission, without original-source authority.
    pub fn check_transport<S>(
        &self,
        abi: &FnAbi,
        unit: &LirUnit<S>,
        layouts: &LayoutTable,
    ) -> Result<(), &'static str> {
        self.check_inner(abi, unit, layouts, None, true)
    }

    fn check_inner<S>(
        &self,
        abi: &FnAbi,
        unit: &LirUnit<S>,
        layouts: &LayoutTable,
        source: Option<&crate::sealed_payload_source::ReceiptSource>,
        transport: bool,
    ) -> Result<(), &'static str> {
        crate::abi::check_receipt_contract(abi, layouts)?;
        let contract = abi
            .receipts
            .as_ref()
            .ok_or("receipt entry has no callee contract")?;
        if self.contract != contract.content()? {
            return Err("receipt entry names a different complete contract");
        }
        if let Some(environment) = self.environment {
            let (base, suffix) = if transport {
                crate::receipt_closure::transport_environment(layouts, environment, contract)?
            } else if contract
                .captures
                .iter()
                .any(|c| c.mode == crate::sealed_payload_source::ReceiptMode::Take)
            {
                crate::receipt_closure::owned_environment(
                    layouts,
                    environment,
                    source
                        .ok_or("owning receipt entry needs complete original capture profiles")?,
                )?
            } else {
                crate::receipt_closure::borrowed_environment(layouts, environment, contract)?
            };
            self.check_capture_storage(base, unit, layouts)?;
            for ((binding, capture), field) in
                self.captures.iter().zip(&contract.captures).zip(suffix)
            {
                let metadata = unit
                    .slots
                    .get(binding.slot.index())
                    .and_then(|slot| unit.types.get(slot.ty.index()))
                    .ok_or("receipt entry has a foreign metadata slot")?;
                if capture.mode == crate::sealed_payload_source::ReceiptMode::Take
                    && metadata.layout != field.layout
                {
                    return Err("receipt entry metadata differs from its typed owning field");
                }
            }
        }
        if transport && !contract.captures.is_empty() && self.environment.is_none() {
            return Err("receipt capture entry lacks its complete environment");
        }
        if self.parameters.len() != contract.parameters.len()
            || self.captures.len() != contract.captures.len()
            || self.result.is_some() != contract.result.is_some()
        {
            return Err("receipt entry coverage differs from its contract");
        }
        let slots: BTreeSet<_> = self
            .parameters
            .iter()
            .chain(&self.captures)
            .map(|p| p.slot)
            .chain(self.result)
            .collect();
        if slots.len()
            != self.parameters.len() + self.captures.len() + usize::from(self.result.is_some())
            || unit.slots.iter().enumerate().any(|(id, slot)| {
                matches!(
                    slot.kind,
                    SlotKind::ReceiptParameter(_)
                        | SlotKind::ReceiptCapture(_)
                        | SlotKind::ReceiptResult
                ) && !slots.contains(&SlotId(id as u32))
            })
        {
            return Err("receipt entry slot namespace differs from its complete bindings");
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
            if let Some(source) = source {
                let profile = source
                    .profile(crate::sealed_payload_source::ReceiptPort::Capture(
                        binding.source,
                    ))
                    .ok_or("receipt entry lacks its original capture profile")?;
                let metadata = unit.types[unit.slots[binding.slot.index()].ty.index()].layout;
                if !matches!(layouts.layout(metadata).map(|r| &r.kind), Some(LayoutKind::PayloadReceipt { root }) if root == &profile.root.ty)
                {
                    return Err("receipt entry metadata describes another original capture type");
                }
            }
        }
        if let Some(result) = self.result {
            Self::slot(result, SlotKind::ReceiptResult, unit, layouts)?;
        }
        Ok(())
    }

    /// The complete base includes the once live word, represented by its own
    /// final Capture slot. Metadata companions never replace an ordinary field.
    fn check_capture_storage<S>(
        &self,
        base: LayoutId,
        unit: &LirUnit<S>,
        layouts: &LayoutTable,
    ) -> Result<(), &'static str> {
        let base = layouts
            .layout(base)
            .ok_or("receipt entry base is missing")?;
        let mut slots: Vec<_> = unit
            .slots
            .iter()
            .filter_map(|slot| {
                if let SlotKind::Capture(ordinal) = slot.kind {
                    Some((ordinal, slot.ty))
                } else {
                    None
                }
            })
            .collect();
        slots.sort_by_key(|s| s.0);
        if slots.len() != base.fields.len() || slots.len() != unit.captures.len() {
            return Err("receipt entry base differs from ordinary capture storage");
        }
        let mut offset = 0u64;
        let mut align = 1u32;
        for (index, ((ordinal, ty), field)) in slots.iter().zip(&base.fields).enumerate() {
            let capture = unit
                .captures
                .get(index)
                .and_then(|id| unit.values.get(id.index()))
                .ok_or("receipt entry has no ordinary capture version")?;
            let ty_record = unit
                .types
                .get(ty.index())
                .ok_or("receipt entry has a foreign ordinary capture type")?;
            let child = layouts
                .layout(ty_record.layout)
                .ok_or("receipt entry has a foreign ordinary capture layout")?;
            if !child.align.is_power_of_two() {
                return Err("receipt entry has an invalid ordinary capture alignment");
            }
            offset = crate::layout::align_up(offset, u64::from(child.align))
                .map_err(|_| "receipt entry capture offset overflow")?;
            if *ordinal as usize != index
                || field.ordinal != *ordinal
                || capture.ty != *ty
                || field.layout != ty_record.layout
                || field.offset != offset
                || field.unaligned
            {
                return Err("receipt entry base field disagrees with its ordinary capture slot");
            }
            offset = offset
                .checked_add(
                    child
                        .size
                        .storage()
                        .ok_or("receipt entry capture has no finite storage")?,
                )
                .ok_or("receipt entry capture extent overflow")?;
            align = align.max(child.align);
        }
        let size = crate::layout::align_up(offset, u64::from(align))
            .map_err(|_| "receipt entry capture extent overflow")?;
        if base.align != align || base.size != crate::layout_types::SizeSpec::Fixed(size) {
            return Err("receipt entry base extent differs from its ordinary capture storage");
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
} trailing_optional environment);
