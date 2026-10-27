//! Private capture companions. Local schema/layout consistency is not
//! original-source authority, value/receipt pairing or executable custody proof.
use crate::abi_types::FnAbi;
use crate::ir::*;
use crate::layout_types::*;
use crate::sealed_payload_source::{ReceiptContract, ReceiptMode};
use hirsel_codec::snapshot_struct;
use hirsel_syntax::ContentHash;

mod owned;
pub use owned::{owned_environment, owned_environment_key, transport_environment};

#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct ReceiptClosureCapture {
    pub ordinal: u32,
    pub receipt: ValueId,
}

#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct ReceiptClosure {
    pub contract: ContentHash,
    pub captures: Vec<ReceiptClosureCapture>,
}

impl ReceiptClosure {
    pub(crate) fn visit(&self, visit: &mut impl FnMut(Ref)) {
        for capture in &self.captures {
            visit(Ref::Value(capture.receipt));
        }
    }
    pub fn remap_values(&mut self, map: &impl Fn(ValueId) -> ValueId) {
        for capture in &mut self.captures {
            capture.receipt = map(capture.receipt);
        }
    }

    pub fn check<S>(
        &self,
        site: &ClosureSite,
        abi: &FnAbi,
        unit: &LirUnit<S>,
        layouts: &LayoutTable,
    ) -> Result<(), &'static str> {
        self.check_inner(site, abi, unit, layouts, None, false)
    }

    /// Validate the owning schema against complete original capture profiles.
    /// A current source provider and independent C/H pairing remain required.
    pub fn check_with_source<S>(
        &self,
        site: &ClosureSite,
        abi: &FnAbi,
        unit: &LirUnit<S>,
        layouts: &LayoutTable,
        source: &crate::sealed_payload_source::ReceiptSource,
    ) -> Result<(), &'static str> {
        source.check_structure()?;
        let symbol = site
            .code
            .and_then(|code| unit.tables.symbols.get(code.symbol.index()))
            .ok_or("receipt closure lacks its selected source code")?;
        if symbol.kind != SymbolKind::Function
            || symbol.definition != source.original.definition
            || symbol.instance != Some(source.instance)
        {
            return Err("receipt closure code differs from its selected original source");
        }
        if abi.receipts.as_ref() != Some(&source.contract) {
            return Err("receipt closure source differs from its complete code contract");
        }
        self.check_inner(site, abi, unit, layouts, Some(source), false)
    }

    /// Physical emitter checks after admission, without original-source authority.
    pub fn check_transport<S>(
        &self,
        site: &ClosureSite,
        abi: &FnAbi,
        unit: &LirUnit<S>,
        layouts: &LayoutTable,
    ) -> Result<(), &'static str> {
        self.check_inner(site, abi, unit, layouts, None, true)
    }

    fn check_inner<S>(
        &self,
        site: &ClosureSite,
        abi: &FnAbi,
        unit: &LirUnit<S>,
        layouts: &LayoutTable,
        source: Option<&crate::sealed_payload_source::ReceiptSource>,
        transport: bool,
    ) -> Result<(), &'static str> {
        crate::abi::check_receipt_contract(abi, layouts)?;
        if !site.scoped
            || site
                .code
                .is_none_or(|code| layouts.fn_abis.get(code.abi.0 as usize) != Some(abi))
        {
            return Err("receipt closure requires its actual scoped code ABI");
        }
        let contract = abi
            .receipts
            .as_ref()
            .ok_or("receipt closure lacks its code contract")?;
        if self.contract != contract.content()? {
            return Err("receipt closure names another complete code contract");
        }
        if self.captures.len() != contract.captures.len() {
            return Err("receipt closure capture coverage differs");
        }
        let owned = contract
            .captures
            .iter()
            .any(|c| c.mode == ReceiptMode::Take);
        let (base, suffix) = if transport {
            transport_environment(layouts, site.environment, contract)?
        } else if owned {
            owned_environment(
                layouts,
                site.environment,
                source.ok_or("owning receipt closure needs complete original capture profiles")?,
            )?
        } else {
            borrowed_environment(layouts, site.environment, contract)?
        };
        let ordinary = site
            .captures
            .get(&unit.pools.captures)
            .ok_or("receipt closure lacks ordinary capture operands")?;
        let base = layouts
            .layout(base)
            .ok_or("receipt closure lacks its base environment")?;
        let count = base.fields.len().saturating_sub(usize::from(matches!(
            base.kind,
            LayoutKind::Environment { .. }
        )));
        if count != ordinary.len() {
            return Err("receipt closure base differs from its original captures");
        }
        for ((binding, demanded), field) in self.captures.iter().zip(&contract.captures).zip(suffix)
        {
            if binding.ordinal != demanded.ordinal {
                return Err("receipt closure capture selectors are not canonical");
            }
            let capture = ordinary
                .get(binding.ordinal as usize)
                .ok_or("receipt closure names a foreign original capture")?;
            let owned = match demanded.mode {
                ReceiptMode::Borrow if capture.usage == CaptureUse::Borrow => false,
                ReceiptMode::Take
                    if matches!(capture.usage, CaptureUse::Copy | CaptureUse::Take) =>
                {
                    true
                }
                _ => return Err("receipt closure capture usage differs from metadata custody"),
            };
            let ordinary = unit
                .values
                .get(capture.value.index())
                .and_then(|v| unit.types.get(v.ty.index()))
                .ok_or("receipt closure has a foreign ordinary capture type")?;
            if base.fields[binding.ordinal as usize].layout != ordinary.layout {
                return Err("receipt closure base differs from its ordinary capture type");
            }
            let value = unit
                .values
                .get(binding.receipt.index())
                .ok_or("receipt closure names a foreign receipt value")?;
            let ty = unit
                .types
                .get(value.ty.index())
                .ok_or("receipt closure has a foreign receipt type")?;
            if (owned && field.layout != ty.layout)
                || ty.repr != ValueRepr::PayloadReceipt
                || if owned {
                    value.holding != Holding::Owned
                } else {
                    !matches!(value.holding, Holding::Borrowed(_))
                }
                || !layouts
                    .layout(ty.layout)
                    .is_some_and(|r| matches!(r.kind, LayoutKind::PayloadReceipt { .. }))
            {
                return Err("receipt closure companion has another metadata custody");
            }
            if let Some(source) = source {
                let profile = source
                    .profile(crate::sealed_payload_source::ReceiptPort::Capture(
                        demanded.ordinal,
                    ))
                    .ok_or("receipt closure lacks its original capture profile")?;
                if !matches!(layouts.layout(ty.layout).map(|r| &r.kind), Some(LayoutKind::PayloadReceipt { root }) if root == &profile.root.ty)
                {
                    return Err(
                        "receipt closure companion describes another original capture type",
                    );
                }
            }
        }
        Ok(())
    }
}

/// Content identity of an environment wrapper, retaining the complete contract
/// rather than only the erased native capture shape.
pub fn environment_key(base: &LayoutIdentity, contract: ContentHash) -> LayoutIdentity {
    LayoutIdentity {
        owner_revision: None,
        instance: Some(crate::snapshot::digest(
            "borrowed-receipt-environment-v1",
            &(base.clone(), contract),
        )),
        target: base.target,
        layout_version: crate::LAYOUT_VERSION,
    }
}

/// Independently check the physical wrapper's base and pointer companion suffix.
/// The complete ordinary environment, including a once live word, stays at zero.
pub fn borrowed_environment<'a>(
    layouts: &'a LayoutTable,
    environment: LayoutId,
    contract: &ReceiptContract,
) -> Result<(LayoutId, &'a [FieldLayout]), &'static str> {
    contract.check_structure()?;
    if contract.captures.is_empty()
        || contract
            .captures
            .iter()
            .any(|c| c.mode != ReceiptMode::Borrow)
    {
        return Err("receipt environment requires nonempty borrowed capture demand");
    }
    let record = layouts
        .layout(environment)
        .ok_or("receipt environment layout is missing")?;
    let Some(first) = record.fields.first() else {
        return Err("receipt environment has no original base");
    };
    let base = layouts
        .layout(first.layout)
        .ok_or("receipt environment base layout is missing")?;
    if record.kind != LayoutKind::Aggregate
        || first.ordinal != 0
        || first.offset != 0
        || first.unaligned
        || !matches!(
            base.kind,
            LayoutKind::Aggregate | LayoutKind::Environment { .. }
        )
        || record.key != environment_key(&base.key, contract.content()?)
        || record.fields.len() != contract.captures.len() + 1
    {
        return Err("receipt environment wrapper disagrees with its base or complete contract");
    }
    let ordinary_count = base.fields.len().saturating_sub(usize::from(matches!(
        base.kind,
        LayoutKind::Environment { .. }
    )));
    if contract
        .captures
        .iter()
        .any(|c| c.ordinal as usize >= ordinary_count)
    {
        return Err("receipt environment names a foreign original capture");
    }
    let SizeSpec::Fixed(mut offset) = base.size else {
        return Err("receipt environment base is not fixed size");
    };
    let mut align = base.align;
    for (index, field) in record.fields[1..].iter().enumerate() {
        let child = layouts
            .layout(field.layout)
            .ok_or("receipt environment companion layout is missing")?;
        if child.kind != LayoutKind::Scalar(ScalarKind::Pointer)
            || !child.align.is_power_of_two()
            || child.copy != CopyRecipe::Bits
            || !child.scan.is_empty()
            || !child.drop.is_empty()
            || child.finalizer.is_some()
        {
            return Err("receipt environment companion is not a nonowning pointer");
        }
        offset = crate::layout::align_up(offset, u64::from(child.align))
            .map_err(|_| "receipt environment offset overflow")?;
        if field.ordinal as usize != index + 1 || field.offset != offset || field.unaligned {
            return Err("receipt environment companion offsets are not canonical");
        }
        let SizeSpec::Fixed(size) = child.size else {
            return Err("receipt environment pointer is not fixed size");
        };
        offset = offset
            .checked_add(size)
            .ok_or("receipt environment extent overflow")?;
        align = align.max(child.align);
    }
    let size = crate::layout::align_up(offset, u64::from(align))
        .map_err(|_| "receipt environment extent overflow")?;
    if record.align != align
        || record.size != SizeSpec::Fixed(size)
        || record.finalizer.is_some()
        || record.tag.is_some()
    {
        return Err("receipt environment has another extent or finalizer");
    }
    Ok((first.layout, &record.fields[1..]))
}

snapshot_struct!(ReceiptClosureCapture { ordinal, receipt });
snapshot_struct!(ReceiptClosure { contract, captures });
