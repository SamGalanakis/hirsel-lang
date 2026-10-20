//! SEM-4 pull iterators: owned environments and ordinary indirect step calls.
use super::builder::{Body, Parameter};
use super::{CallShape, Cx, InlineLowering, LibraryBody, R, native_path};
use crate::PassError;
use crate::import::ImportCx;
use hirsel_core_contract::Passing;
use hirsel_lir::abi_types::{CallableShape, FnAbiId};
use hirsel_lir::ir::*;
use hirsel_lir::layout_types::LayoutId;
use hirsel_lir::stage::Value;
use hirsel_types::{TypeKind, UseKind};

pub static FROM: InlineLowering = InlineLowering {
    path: || hirsel_syntax::DefPath::core("iter.from"),
    lower: from,
};
pub static NEXT: InlineLowering = InlineLowering {
    path: || hirsel_syntax::DefPath::core("iter.next"),
    lower: next,
};
pub static FROM_STEP: LibraryBody = LibraryBody {
    path: || native_path("iter_from_step"),
    build: from_step,
};

fn construct(cx: &mut ImportCx<'_>, call: &CallShape<'_>, name: &str) -> R<bool> {
    if call.generics.len() != 2
        || call.arguments.len() != 2
        || call
            .arguments
            .iter()
            .any(|a| a.value.usage != UseKind::Take)
    {
        return Ok(false);
    }
    let result = cx.iterator_closure(call, &native_path(name))?;
    cx.finish(call, result)?;
    Ok(true)
}
fn from(cx: &mut ImportCx<'_>, call: &CallShape<'_>) -> R<bool> {
    construct(cx, call, "iter_from_step")
}
fn next(cx: &mut ImportCx<'_>, call: &CallShape<'_>) -> R<bool> {
    if call.arguments.len() != 1 || call.arguments[0].value.usage != UseKind::Mut {
        return Ok(false);
    }
    cx.iterator_next(call)?;
    Ok(true)
}

fn captures(b: &mut Body<'_, '_>) -> Result<[Parameter; 2], PassError> {
    let environment =
        b.cx.environment()
            .ok_or_else(|| PassError::Malformed("iterator thunk without environment".into()))?;
    let fields = b.record(environment).fields.clone();
    Ok([
        b.capture(0, fields[0].layout)?,
        b.capture(1, fields[1].layout)?,
    ])
}
fn callback_abi(b: &mut Body<'_, '_>, ordinal: usize) -> Result<FnAbiId, PassError> {
    let ty = b.cx.type_actual(ordinal)?;
    b.cx.layouts
        .fn_abi(&b.cx.store, ty, CallableShape::Closure)
        .map_err(|e| PassError::Layout(b.cx.path.clone(), e))
}

/// An indirect call of `callee` (`Body::call`).
pub(crate) fn call(
    b: &mut Body<'_, '_>,
    callee: ValueId,
    abi: FnAbiId,
    arguments: Vec<CallArgument>,
    loans: &[LoanId],
) -> Result<Option<ValueId>, PassError> {
    Ok(b.call(Callee::Indirect(callee), Vec::new(), abi, arguments, loans))
}

fn from_step(cx: &mut Cx<'_>) -> Result<LirUnit<Value>, PassError> {
    let mut b = Body::new(cx)?;
    let [state, next] = captures(&mut b)?;
    let abi = callback_abi(&mut b, 1)?;
    let (callee, loan) = b.borrow(next.address, false)?;
    let version = b.version(state.slot);
    b.set_reaching(state.slot, version);
    let result = call(
        &mut b,
        callee,
        abi,
        vec![CallArgument {
            parameter: 0,
            passing: Passing::Mut,
            value: ArgumentValue::Address {
                address: state.address,
                version,
            },
        }],
        &[loan],
    )?;
    b.ret(result);
    Ok(b.finish())
}

pub(crate) fn option(
    b: &mut Body<'_, '_>,
    layout: LayoutId,
    variant: u32,
    values: &[ValueId],
) -> ValueId {
    let definition = b.symbol(Symbol {
        kind: SymbolKind::Type,
        definition: hirsel_syntax::DefPath::core("Option"),
        instance: None,
    });
    let fields = PoolRange::push(
        &mut b.ed.unit.pools.fields,
        values.iter().enumerate().map(|(i, &value)| FieldValue {
            field: i as u32,
            value,
        }),
    );
    let construct = ConstructId(b.ed.unit.tables.constructs.len() as u32);
    b.ed.unit.tables.constructs.push(ConstructSite {
        arena_no_drop: false,
        tracked: false,
        constructor: ConstructorKind::Variant {
            definition,
            variant,
        },
        layout,
        fields,
        destination: DestinationClass::Inferred,
        placement: None,
        anchor: b.anchor,
    });
    let result = b.fresh(layout);
    let failure = b.failure(None);
    b.push(Inst::Construct {
        result,
        construct,
        failure,
    });
    result
}
pub(crate) fn local(
    b: &mut Body<'_, '_>,
    value: ValueId,
) -> Result<(SlotId, AddressId), PassError> {
    let layout = b.layout_of(value);
    let slot = b.slot(layout, SlotKind::Temporary);
    b.push(Inst::StorageLive { slot });
    let ty = b.ty(layout);
    let address = AddressId(b.ed.unit.addresses.len() as u32);
    b.ed.unit.addresses.push(AddressInfo {
        source_place: None,
        unchecked: Vec::new(),
        base: AddressBase::Slot(slot),
        projection: PoolRange::EMPTY,
        ty,
    });
    // A local's address is prepared at entry before any produced owner exists,
    // just as source import prepares fixed frame storage in its prologue; its
    // failure releases only the owned parameters held there.
    let failure = b.entry_failure();
    b.ed.code[b.ed.unit.entry.index()].push(Inst::Address { address, failure });
    b.store(address, value)?;
    b.hold(slot, address);
    Ok((slot, address))
}
pub(crate) fn drop_local(b: &mut Body<'_, '_>, slot: SlotId, address: AddressId) {
    b.emptied(slot);
    let failure = b.failure(None);
    let anchor = CleanupAnchorId(b.ed.unit.tables.cleanup_anchors.len() as u32);
    b.ed.unit.tables.cleanup_anchors.push(CleanupAnchor {
        body: 0,
        cleanup: 0,
        action: 0,
        site: SiteRange { start: 0, end: 0 },
    });
    b.push(Inst::Drop {
        address,
        if_live: false,
        anchor,
        failure,
    });
    b.push(Inst::StorageDead {
        slot,
        if_live: false,
    });
}
