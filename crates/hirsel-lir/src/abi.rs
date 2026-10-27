//! Physical ABI arithmetic, digests and runtime symbol resolution.
use crate::abi_types::*;
use crate::layout_types::{LayoutId, ScalarKind, TargetSpec};
use crate::snapshot;
use hirsel_syntax::ContentHash;

/// Local consistency only. Original selectors, closed profiles and complete
/// demand still require independently current source declarations in C.
pub fn check_receipt_contract(
    abi: &FnAbi,
    layouts: &crate::LayoutTable,
) -> Result<(), &'static str> {
    use crate::sealed_payload_source::{ReceiptGrant, ReceiptMode};
    let Some(contract) = &abi.receipts else {
        return if abi.key.receipt_contract.is_none() {
            Ok(())
        } else {
            Err("receipt ABI key without its complete contract")
        };
    };
    contract.check_structure()?;
    if abi.shape == CallableShape::Foreign {
        return Err("foreign entries require Hirsel receipt adapters");
    }
    if abi.key.receipt_contract != Some(contract.content()?) {
        return Err("receipt ABI key disagrees with its complete contract");
    }
    for demanded in &contract.parameters {
        let Some(actual) = abi.parameters.iter().find(|p| p.source == demanded.source) else {
            return Err("receipt parameter is not an original declared parameter");
        };
        let valid = match demanded.mode {
            ReceiptMode::Borrow => actual.convention == hirsel_core_contract::Passing::Borrow,
            ReceiptMode::Take => actual.convention == hirsel_core_contract::Passing::Take,
            ReceiptMode::MutSlot => {
                actual.convention == hirsel_core_contract::Passing::Mut
                    && actual.passing == ParameterPassing::ExclusiveAddress
            }
        };
        if !valid {
            return Err("receipt passing mode disagrees with ordinary parameter");
        }
    }
    if contract.result.is_some() && abi.diverges {
        return Err("diverging entries cannot publish a receipt result");
    }
    if !contract.captures.is_empty() && !abi.environment {
        return Err("capture receipts require an actual environment contract");
    }
    for grant in &contract.inventory {
        let valid = match grant {
            ReceiptGrant::Parameter(source) => abi
                .parameters
                .iter()
                .find(|p| p.source == *source)
                .and_then(|p| layouts.layouts.get(p.layout.0 as usize))
                .is_some_and(|l| l.kind == crate::layout_types::LayoutKind::Arena),
            ReceiptGrant::Capture(_) => abi.environment,
            ReceiptGrant::CallerDestination => abi.destination,
        };
        if !valid {
            return Err("receipt inventory selector lacks its arena or environment carrier");
        }
    }
    Ok(())
}

/// Ready/H/backend receipt transport is not connected yet. Structural ABI validation
/// alone cannot authorize dropping carriers or mint original-source evidence.
/// Ordinary rows retain their existing implementation in C, H and both emitters.
pub fn check_implemented_transport(abi: &FnAbi) -> Result<(), &'static str> {
    if abi.receipts.is_some() || abi.key.receipt_contract.is_some() {
        return Err("receipt ABI transport requires independent source and carrier validation");
    }
    Ok(())
}

/// One appended parameter carrier in the physical Hirsel signature.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReceiptParameterSlot {
    pub source: u32,
    pub mode: crate::sealed_payload_source::ReceiptMode,
    pub index: usize,
}

/// Receipt carriers follow the entire ordinary physical argument prefix.
/// Captures remain in the demanded environment; unit results still have an
/// output slot, and register-returned values keep their ordinary convention.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ReceiptSlots {
    pub parameters: Vec<ReceiptParameterSlot>,
    pub result: Option<usize>,
}

impl ReceiptSlots {
    pub fn len(&self) -> usize {
        self.parameters.len() + usize::from(self.result.is_some())
    }
    pub fn is_empty(&self) -> bool {
        self.parameters.is_empty() && self.result.is_none()
    }
}

/// Derive only physical carrier positions. Complete source authority and
/// value/receipt custody are checked separately before executable acceptance.
pub fn receipt_slots(abi: &FnAbi, first: usize) -> Result<ReceiptSlots, &'static str> {
    let Some(contract) = &abi.receipts else {
        return if abi.key.receipt_contract.is_none() {
            Ok(ReceiptSlots::default())
        } else {
            Err("receipt ABI key without its complete contract")
        };
    };
    if abi.shape == CallableShape::Foreign {
        return Err("foreign entries require Hirsel receipt adapters");
    }
    if abi.key.receipt_contract != Some(contract.content()?) {
        return Err("receipt ABI key disagrees with its complete contract");
    }
    first
        .checked_add(contract.parameters.len())
        .and_then(|end| end.checked_add(usize::from(contract.result.is_some())))
        .ok_or("receipt carrier index overflow")?;
    let mut next = first;
    let mut parameters = Vec::with_capacity(contract.parameters.len());
    for parameter in &contract.parameters {
        parameters.push(ReceiptParameterSlot {
            source: parameter.source,
            mode: parameter.mode,
            index: next,
        });
        next = next
            .checked_add(1)
            .ok_or("receipt carrier index overflow")?;
    }
    let result = contract.result.as_ref().map(|_| next);
    if result.is_some() && abi.diverges {
        return Err("diverging entries cannot publish a receipt result");
    }
    Ok(ReceiptSlots { parameters, result })
}

/// The value word of a typed Hirsel result, when it fits beside its status.
/// Wide integers and inline aggregates retain the output pointer.
pub fn register_kind(kind: &crate::layout_types::LayoutKind) -> Option<ScalarKind> {
    crate::emission::scalar_shape(kind)
        .filter(|kind| !matches!(kind, ScalarKind::U128 | ScalarKind::I128))
}

/// The physical ABI digest: everything callers and callees must agree on,
/// without table-local layout indices. Equal digests mean interchangeable
/// entries.
pub fn abi_digest(abi: &FnAbi) -> ContentHash {
    let mut physical = abi.clone();
    // Body placement proofs do not change the callable machine convention.
    physical.result_arena = None;
    for parameter in &mut physical.parameters {
        parameter.layout = LayoutId(0);
    }
    if let Some(result) = &mut physical.result {
        result.layout = LayoutId(0);
    }
    snapshot::digest("fn-abi-physical", &physical)
}

/// Contract identity is part of serialized and linked callable identity, even
/// when two entries have exactly the same machine slots.
pub fn scoped_contract_digest(
    parameters: &[ParameterPath],
    captures: &[ParameterPath],
) -> ContentHash {
    snapshot::digest(
        "scoped-contract-v1",
        &(parameters.to_vec(), captures.to_vec()),
    )
}

/// The physical slot of a scalar on `target`.
pub fn slot(target: &TargetSpec, scalar: ScalarKind) -> PhysicalSlot {
    PhysicalSlot {
        ty: physical(scalar),
        size: target.scalar_size(scalar),
        align: target.scalar_align(scalar),
    }
}

pub fn physical(scalar: ScalarKind) -> PhysicalType {
    match scalar {
        ScalarKind::Bool | ScalarKind::U8 | ScalarKind::I8 => PhysicalType::I8,
        ScalarKind::U16 | ScalarKind::I16 => PhysicalType::I16,
        ScalarKind::U32 | ScalarKind::I32 => PhysicalType::I32,
        ScalarKind::U64 | ScalarKind::I64 => PhysicalType::I64,
        ScalarKind::U128 | ScalarKind::I128 => PhysicalType::I128,
        ScalarKind::F32 => PhysicalType::F32,
        ScalarKind::F64 => PhysicalType::F64,
        ScalarKind::Pointer => PhysicalType::Pointer,
    }
}

/// Narrow integers are extended to a full register by the caller.
pub fn extension_of(scalar: ScalarKind) -> ScalarExtension {
    match scalar {
        ScalarKind::Bool | ScalarKind::U8 | ScalarKind::U16 => ScalarExtension::Zero,
        ScalarKind::I8 | ScalarKind::I16 => ScalarExtension::Sign,
        _ => ScalarExtension::None,
    }
}

/// The link name a `RuntimePrimitive` symbol template names for one call
/// (LIR schema revision 6). A template names a monomorphic family by
/// placeholders: `{I}` is the kind of the first scalar argument and `{J}` the
/// result's, both integers; `{F}` is the float kind of the first scalar
/// argument, else of the result (an array reduction takes an array). A name is
/// spelled as its scalar kind (`u8` .. `i128`, `f32`, `f64`). None when a
/// placeholder has no kind of its domain, is unknown, or a brace is
/// unbalanced: such a row has no native symbol for this call.
pub fn resolve_symbol(
    template: &str,
    arguments: &[ScalarKind],
    result: Option<ScalarKind>,
) -> Option<String> {
    let integer = |k: Option<ScalarKind>| k.filter(|k| scalar_name(*k).is_some() && !float(*k));
    let floating = |k: Option<ScalarKind>| k.filter(|k| float(*k));
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(open) = rest.find(['{', '}']) {
        out.push_str(&rest[..open]);
        if !rest[open..].starts_with('{') {
            return None;
        }
        let close = rest[open..].find('}')? + open;
        let kind = match &rest[open + 1..close] {
            "I" => integer(arguments.first().copied())?,
            "J" => integer(result)?,
            "F" => floating(arguments.first().copied()).or(floating(result))?,
            _ => return None,
        };
        out.push_str(scalar_name(kind)?);
        rest = &rest[close + 1..];
    }
    out.push_str(rest);
    Some(out)
}

/// `resolve_symbol` for a call site of `unit`: the scalar kinds of its
/// value arguments in parameter order and of its result.
pub fn resolve_call_symbol<S>(
    template: &str,
    unit: &crate::ir::LirUnit<S>,
    site: &crate::ir::CallSite,
) -> Option<String> {
    use crate::ir::{ArgumentValue, ValueId, ValueRepr};
    let scalar = |v: ValueId| match unit.types[unit.values[v.index()].ty.index()].repr {
        ValueRepr::Scalar(k) => Some(k),
        _ => None,
    };
    let mut arguments = site
        .arguments
        .get(&unit.pools.call_arguments)
        .unwrap_or(&[])
        .to_vec();
    arguments.sort_by_key(|a| a.parameter);
    let kinds: Vec<ScalarKind> = arguments
        .iter()
        .filter_map(|a| match a.value {
            ArgumentValue::Value(v) => scalar(v),
            ArgumentValue::Address { .. } => None,
        })
        .collect();
    resolve_symbol(template, &kinds, site.result.and_then(scalar))
}

fn float(kind: ScalarKind) -> bool {
    matches!(kind, ScalarKind::F32 | ScalarKind::F64)
}

fn scalar_name(kind: ScalarKind) -> Option<&'static str> {
    Some(match kind {
        ScalarKind::U8 => "u8",
        ScalarKind::U16 => "u16",
        ScalarKind::U32 => "u32",
        ScalarKind::U64 => "u64",
        ScalarKind::U128 => "u128",
        ScalarKind::I8 => "i8",
        ScalarKind::I16 => "i16",
        ScalarKind::I32 => "i32",
        ScalarKind::I64 => "i64",
        ScalarKind::I128 => "i128",
        ScalarKind::F32 => "f32",
        ScalarKind::F64 => "f64",
        ScalarKind::Bool | ScalarKind::Pointer => return None,
    })
}

/// The platform C register of a foreign value. Nullable pointers retain their
/// ordinary variant layout internally; the C boundary reads/writes its niche word.
pub fn foreign_scalar(table: &crate::LayoutTable, layout: LayoutId) -> Option<ScalarKind> {
    use crate::layout_types::{LayoutKind, TagForm};
    let record = table.layout(layout)?;
    if let LayoutKind::Scalar(kind) = record.kind {
        return Some(kind);
    }
    let tag = record.tag.as_ref()?;
    let TagForm::Niche { dataful, .. } = tag.form else {
        return None;
    };
    let fields = &tag.variants.get(dataful as usize)?.fields;
    (tag.offset == 0
        && matches!(fields.as_slice(), [f] if f.offset == 0
            && table.layout(f.layout)?.kind == LayoutKind::Scalar(ScalarKind::Pointer)
            && record.size == table.layout(f.layout)?.size))
    .then_some(ScalarKind::Pointer)
}

/// Non-null pointer words C can overwrite through a writable record address.
/// A nullable variant's niche word is deliberately absent.
pub fn foreign_nonnull_offsets(table: &crate::LayoutTable, layout: LayoutId) -> Vec<u64> {
    use crate::layout_types::LayoutKind;
    fn visit(table: &crate::LayoutTable, layout: LayoutId, base: u64, out: &mut Vec<u64>) {
        let Some(record) = table.layout(layout) else {
            return;
        };
        match record.kind {
            LayoutKind::Scalar(ScalarKind::Pointer) => out.push(base),
            LayoutKind::Aggregate => {
                for field in &record.fields {
                    visit(table, field.layout, base + field.offset, out);
                }
            }
            _ => {}
        }
    }
    let mut offsets = Vec::new();
    visit(table, layout, 0, &mut offsets);
    offsets
}

/// Invocation identity, independent of local ABI/layout ordinals, closure data,
/// symbol names, capture storage and caller-site decoration. Bodies are checked
/// under these symbolic parameter contracts; complete C authenticates actuals.
pub fn scoped_entry_identity(abi: &FnAbi, layouts: &crate::LayoutTable) -> ContentHash {
    scoped_entry_identity_with_keys(abi, &crate::layout_keys::LayoutKeys::new(layouts))
}

pub(crate) fn scoped_entry_identity_with_keys(
    abi: &FnAbi,
    keys: &crate::layout_keys::LayoutKeys,
) -> ContentHash {
    let parameters: Vec<_> = abi
        .parameters
        .iter()
        .map(|p| ((p.convention, p.passing), keys.key(p.layout)))
        .collect();
    let result = abi
        .result
        .as_ref()
        .map(|r| (keys.key(r.layout), r.borrowed_parameters.clone()));
    let signature = snapshot::digest("scoped-entry-signature-v1", &(parameters, result));
    let contracts = snapshot::digest(
        "scoped-entry-contracts-v1",
        &(abi.nonretaining.clone(), abi.scoped_entries.clone()),
    );
    let receipt = abi
        .receipts
        .as_ref()
        .map(|contract| snapshot::digest("scoped-entry-receipts-v1", contract));
    let modes = [abi.multiplicity as u8, u8::from(abi.diverges)];
    let mut parts = vec![
        signature.0.as_slice(),
        contracts.0.as_slice(),
        modes.as_slice(),
    ];
    if let Some(receipt) = &receipt {
        parts.push(receipt.0.as_slice());
    }
    ContentHash::of_parts("hirsel.scoped-entry.v1", parts)
}

#[cfg(test)]
mod receipt_slots_tests;
