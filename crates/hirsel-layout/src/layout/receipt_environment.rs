//! Demand-only capture metadata suffixes. The base environment remains one
//! complete inline field, including its existing once live word and recipes.
use super::*;
use hirsel_lir::sealed_payload_source::{ReceiptContract, ReceiptMode, ReceiptPort, ReceiptSource};

impl LayoutBuilder<'_> {
    /// Private companion storage for copied or taken captures. The metadata
    /// suffix owns its handles independently of the ordinary capture fields.
    /// This does not admit capture transport through C, H or native emission.
    pub fn owning_receipt_environment(
        &mut self,
        base: LayoutId,
        source: &ReceiptSource,
    ) -> Result<LayoutId, LayoutError> {
        self.transaction(|b| {
            source.check_structure().map_err(|e| LayoutError::Invalid(e.into()))?;
            let contract = &source.contract;
            if !contract.captures.iter().any(|c| c.mode == ReceiptMode::Take)
                || contract.captures.iter().any(|c| c.mode == ReceiptMode::MutSlot)
            {
                return invalid("owning receipt environment requires owned capture demand without mutable captures");
            }
            let ordinary = b.table.layouts.get(base.0 as usize).ok_or_else(|| LayoutError::Invalid("receipt environment base layout is missing".into()))?;
            if !matches!(ordinary.kind, LayoutKind::Aggregate | LayoutKind::Environment { .. })
                || ordinary.finalizer.is_some()
            {
                return invalid("owning receipt environment base is not an ordinary environment");
            }
            let count = ordinary.fields.len().saturating_sub(usize::from(matches!(ordinary.kind, LayoutKind::Environment { .. })));
            if contract.captures.iter().any(|c| c.ordinal as usize >= count) {
                return invalid("owning receipt environment names a foreign original capture");
            }
            let SizeSpec::Fixed(size) = ordinary.size else {
                return invalid("owning receipt environment base is not fixed size");
            };
            let align = ordinary.align;
            let base_key = ordinary.key.clone();
            let digest = contract.content().map_err(|e| LayoutError::Invalid(e.into()))?;
            let mut key = Encoder { bytes: Vec::new(), revisions: true };
            key.tag("owning-receipt-environment.v1");
            key.tag(&hirsel_codec::to_text(&base_key));
            key.atom(&digest.0);
            if let Some(&id) = b.memo.get(&key.bytes) { return Ok(id); }
            let mut pieces = vec![Piece { layout: base, size, align }];
            for capture in &contract.captures {
                let layout = match capture.mode {
                    ReceiptMode::Take => {
                        let profile = source.profile(ReceiptPort::Capture(capture.ordinal)).ok_or_else(|| LayoutError::Invalid("owning receipt capture profile is missing".into()))?;
                        b.payload_receipt(&profile.root.ty)
                    }
                    ReceiptMode::Borrow => b.scalar(ScalarKind::Pointer),
                    ReceiptMode::MutSlot => unreachable!("checked modes"),
                };
                let record = b.record(layout);
                pieces.push(Piece { layout, size: record.size.storage().ok_or(LayoutError::Overflow)?, align: record.align });
            }
            let mut record = b.aggregate(&key.bytes, pieces, false, None)?;
            record.key = hirsel_lir::receipt_closure::owned_environment_key(&base_key, digest);
            Ok(b.insert(key.bytes, record))
        })
    }

    pub fn borrowed_receipt_environment(
        &mut self,
        base: LayoutId,
        contract: &ReceiptContract,
    ) -> Result<LayoutId, LayoutError> {
        self.transaction(|b| {
            contract
                .check_structure()
                .map_err(|e| LayoutError::Invalid(e.into()))?;
            let ordinary = b.table.layouts.get(base.0 as usize).ok_or_else(|| {
                LayoutError::Invalid("receipt environment base layout is missing".into())
            })?;
            if !matches!(
                ordinary.kind,
                LayoutKind::Aggregate | LayoutKind::Environment { .. }
            ) {
                return invalid("receipt environment base is not an ordinary environment");
            }
            if contract.captures.is_empty() {
                return Ok(base);
            }
            if contract
                .captures
                .iter()
                .any(|c| c.mode != ReceiptMode::Borrow)
            {
                return unavailable(
                    None,
                    "only borrowed receipt capture environments are implemented",
                );
            }
            let count = ordinary.fields.len().saturating_sub(usize::from(matches!(
                ordinary.kind,
                LayoutKind::Environment { .. }
            )));
            if contract
                .captures
                .iter()
                .any(|c| c.ordinal as usize >= count)
            {
                return invalid("receipt environment names a foreign original capture");
            }
            let SizeSpec::Fixed(size) = ordinary.size else {
                return invalid("receipt environment base is not fixed size");
            };
            let align = ordinary.align;
            let base_key = ordinary.key.clone();
            let digest = contract
                .content()
                .map_err(|e| LayoutError::Invalid(e.into()))?;
            let mut key = Encoder {
                bytes: Vec::new(),
                revisions: true,
            };
            key.tag("borrowed-receipt-environment.v1");
            key.tag(&hirsel_codec::to_text(&base_key));
            key.atom(&digest.0);
            if let Some(&id) = b.memo.get(&key.bytes) {
                return Ok(id);
            }
            let pointer = b.scalar(ScalarKind::Pointer);
            let (pointer_size, pointer_align) = b.pointer();
            let mut pieces = vec![Piece {
                layout: base,
                size,
                align,
            }];
            pieces.extend(contract.captures.iter().map(|_| Piece {
                layout: pointer,
                size: pointer_size,
                align: pointer_align,
            }));
            let mut record = b.aggregate(&key.bytes, pieces, false, None)?;
            record.key = hirsel_lir::receipt_closure::environment_key(&base_key, digest);
            Ok(b.insert(key.bytes, record))
        })
    }
}

#[cfg(test)]
mod tests;
