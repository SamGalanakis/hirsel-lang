//! Check owning capture wrappers from complete source profiles. This checks a
//! physical schema, not source provenance or executable value/receipt pairing.
use super::*;
use crate::sealed_payload_source::{ReceiptPort, ReceiptSource};

pub fn owned_environment_key(base: &LayoutIdentity, contract: ContentHash) -> LayoutIdentity {
    LayoutIdentity {
        owner_revision: None,
        instance: Some(crate::snapshot::digest(
            "owning-receipt-environment-v1",
            &(base.clone(), contract),
        )),
        target: base.target,
        layout_version: crate::LAYOUT_VERSION,
    }
}

/// The complete ordinary environment stays at zero. Borrowed companions are
/// inert pointers; owning companions have their exact original metadata type
/// and release only metadata. Neither kind contributes language credits.
pub fn owned_environment<'a>(
    layouts: &'a LayoutTable,
    environment: LayoutId,
    source: &ReceiptSource,
) -> Result<(LayoutId, &'a [FieldLayout]), &'static str> {
    source.check_structure()?;
    if !source
        .contract
        .captures
        .iter()
        .any(|capture| capture.mode == ReceiptMode::Take)
    {
        return Err(
            "owning receipt environment requires owned capture demand without mutable captures",
        );
    }
    let result = transport_environment(layouts, environment, &source.contract)?;
    for (field, capture) in result.1.iter().zip(&source.contract.captures) {
        let profile = source
            .profile(ReceiptPort::Capture(capture.ordinal))
            .ok_or("owning receipt capture profile is missing")?;
        if capture.mode == ReceiptMode::Take
            && !matches!(layouts.layout(field.layout).map(|record| &record.kind),
                Some(LayoutKind::PayloadReceipt { root }) if root == &profile.root.ty)
        {
            return Err("owning receipt companion has another original type or metadata custody");
        }
    }
    Ok(result)
}

/// Physical transport of an already admitted wrapper. This checks geometry and
/// metadata custody only; original type/profile authority remains with C/H.
pub fn transport_environment<'a>(
    layouts: &'a LayoutTable,
    environment: LayoutId,
    contract: &ReceiptContract,
) -> Result<(LayoutId, &'a [FieldLayout]), &'static str> {
    contract.check_structure()?;
    if contract
        .captures
        .iter()
        .all(|c| c.mode == ReceiptMode::Borrow)
    {
        return borrowed_environment(layouts, environment, contract);
    }
    if !contract
        .captures
        .iter()
        .any(|c| c.mode == ReceiptMode::Take)
        || contract
            .captures
            .iter()
            .any(|c| c.mode == ReceiptMode::MutSlot)
    {
        return Err(
            "owning receipt environment requires owned capture demand without mutable captures",
        );
    }
    let record = layouts
        .layout(environment)
        .ok_or("owning receipt environment is missing")?;
    let first = record
        .fields
        .first()
        .ok_or("owning receipt environment has no base")?;
    let base = layouts
        .layout(first.layout)
        .ok_or("owning receipt environment base is missing")?;
    if record.kind != LayoutKind::Aggregate
        || first.ordinal != 0
        || first.offset != 0
        || first.unaligned
        || !matches!(
            base.kind,
            LayoutKind::Aggregate | LayoutKind::Environment { .. }
        )
        || record.key != owned_environment_key(&base.key, contract.content()?)
        || record.fields.len() != contract.captures.len() + 1
        || base.finalizer.is_some()
    {
        return Err("owning receipt environment differs from its complete base or contract");
    }
    let count = base.fields.len().saturating_sub(usize::from(matches!(
        base.kind,
        LayoutKind::Environment { .. }
    )));
    let SizeSpec::Fixed(mut offset) = base.size else {
        return Err("owning receipt environment base is not fixed size");
    };
    let mut align = base.align;
    let mut drop = Vec::new();
    for (index, (field, capture)) in record.fields[1..]
        .iter()
        .zip(&contract.captures)
        .enumerate()
    {
        if capture.ordinal as usize >= count {
            return Err("owning receipt environment names a foreign original capture");
        }
        let child = layouts
            .layout(field.layout)
            .ok_or("owning receipt companion layout is missing")?;
        let owning = capture.mode == ReceiptMode::Take;
        let kind_matches = if owning {
            matches!(child.kind, LayoutKind::PayloadReceipt { .. })
        } else {
            child.kind == LayoutKind::Scalar(ScalarKind::Pointer)
        };
        let expected_drop = if owning {
            vec![DropStep::ReleaseReceipt { offset: 0 }]
        } else {
            vec![]
        };
        if !kind_matches
            || child.copy
                != if owning {
                    CopyRecipe::NotCopyable
                } else {
                    CopyRecipe::Bits
                }
            || child.drop != expected_drop
            || !child.scan.is_empty()
            || child.finalizer.is_some()
            || !child.fields.is_empty()
            || child.tag.is_some()
            || !child.align.is_power_of_two()
        {
            return Err("owning receipt companion has another original type or metadata custody");
        }
        offset = crate::layout::align_up(offset, u64::from(child.align))
            .map_err(|_| "owning receipt companion offset overflow")?;
        if field.ordinal as usize != index + 1 || field.offset != offset || field.unaligned {
            return Err("owning receipt companion offsets are not canonical");
        }
        if owning {
            drop.push(DropStep::ReleaseReceipt { offset });
        }
        offset = offset
            .checked_add(
                child
                    .size
                    .storage()
                    .ok_or("owning receipt companion is not fixed size")?,
            )
            .ok_or("owning receipt environment extent overflow")?;
        align = align.max(child.align);
    }
    drop.reverse();
    drop.extend(base.drop.iter().cloned());
    let size = crate::layout::align_up(offset, u64::from(align))
        .map_err(|_| "owning receipt environment extent overflow")?;
    if record.size != SizeSpec::Fixed(size)
        || record.align != align
        || record.copy != CopyRecipe::NotCopyable
        || record.scan != base.scan
        || record.drop != drop
        || record.finalizer.is_some()
        || record.tag.is_some()
    {
        return Err("owning receipt environment has another extent or ownership recipe");
    }
    Ok((first.layout, &record.fields[1..]))
}
