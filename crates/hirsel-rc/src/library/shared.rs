//! `Shared[T]` roots (MEM-11). A root is a heap block whose `Frozen` payload
//! holds the value and whose header count is the root count: `Dup` and
//! `Release` of the reference add and give up roots, the last release thaws
//! the value and then releases it. Freezing makes every counted block the
//! value reaches immortal; thawing the last root restores their unique counts.
//! `shared.with` lends the frozen value in place: a read-only address through
//! the borrowed root, whose loan keeps the root live for the body's call.
use super::LibraryBody;
use super::builder::{Body, Cx};
use super::slots::{construct, nominal};
use crate::PassError;
use hirsel_core_contract::Passing;
use hirsel_lir::abi_types::CallableShape;
use hirsel_lir::ir::*;
use hirsel_lir::layout_types::{LayoutId, LayoutKind};
use hirsel_lir::stage::Value;
type R<T> = Result<T, PassError>;
pub static FREEZE: LibraryBody = LibraryBody {
    path: || hirsel_syntax::DefPath::core("shared.freeze"),
    build: freeze,
};
pub static THAW: LibraryBody = LibraryBody {
    path: || hirsel_syntax::DefPath::core("shared.thaw"),
    build: thaw,
};
pub static WITH: LibraryBody = LibraryBody {
    path: || hirsel_syntax::DefPath::core("shared.with"),
    build: with,
};
fn freeze(cx: &mut Cx<'_>) -> R<LirUnit<Value>> {
    let mut b = Body::new(cx)?;
    let value = b.take(b.parameters[0].address)?;
    let root = b.fresh(b.cx.fn_abi().result.unwrap().layout);
    b.push(Inst::Freeze {
        result: root,
        value,
    });
    b.ret(Some(root));
    Ok(b.finish())
}
/// The layout of the value a `Shared` root layout freezes.
fn frozen_value(b: &Body<'_, '_>, root: LayoutId, what: &str) -> R<LayoutId> {
    match b.record(root).kind {
        LayoutKind::Shared { payload } => match b.record(payload).kind {
            LayoutKind::Frozen { value } => Ok(value),
            _ => Err(PassError::Malformed(
                "a Shared payload that is not Frozen".into(),
            )),
        },
        _ => Err(PassError::Malformed(format!("{what} of a non-Shared root"))),
    }
}
/// `shared.with(s, body) -> R`: call `body` with a borrowed view of the value
/// frozen in `s`, read in place through the root (field 0 of its `Frozen`
/// payload). No count changes: the view's loan keeps the borrowed root live.
fn with(cx: &mut Cx<'_>) -> R<LirUnit<Value>> {
    let callback = super::tasks::parameter(&cx.store, cx.callable, 1)?;
    let abi = cx
        .layouts
        .fn_abi(&cx.store, callback, CallableShape::Closure)
        .map_err(|e| PassError::Layout(cx.path.clone(), e))?;
    let mut b = Body::new(cx)?;
    let &[s, body] = b.parameters.as_slice() else {
        return Err(PassError::Malformed("shared.with parameters".into()));
    };
    let value_layout = frozen_value(&b, s.layout, "shared.with")?;
    let place = b.place(
        AddressBase::Address(s.address),
        &[ProjectionStep::Field(0)],
        value_layout,
        None,
    )?;
    let (view, view_loan) = b.borrow(place, false)?;
    let (callee, loan) = b.borrow(body.address, false)?;
    let result = super::iter::call(
        &mut b,
        callee,
        abi,
        vec![CallArgument {
            parameter: 0,
            passing: Passing::Borrow,
            value: ArgumentValue::Value(view),
        }],
        &[view_loan, loan],
    )?;
    b.ret(result);
    Ok(b.finish())
}
/// `Ok` with the value when the root is the last, else `Error` with the root.
/// The root count answers alone: no credit but the roots' own keeps the block.
fn thaw(cx: &mut Cx<'_>) -> R<LirUnit<Value>> {
    let mut b = Body::new(cx)?;
    let result = b.cx.fn_abi().result.unwrap().layout;
    let root = b.take(b.parameters[0].address)?;
    let value_layout = frozen_value(&b, b.layout_of(root), "shared.thaw")?;
    let bool_layout = b.bool_layout()?;
    let last = b.value(bool_layout, Holding::Plain);
    b.push(Inst::IsUnique {
        result: last,
        value: root,
    });
    let thawed = b.block(BlockKind::Body, &[]);
    let kept = b.block(BlockKind::Body, &[]);
    b.branch(last, thawed, kept);
    let definition = nominal(&mut b, "Result");
    b.at(thawed);
    let value = b.fresh(value_layout);
    b.push(Inst::Thaw {
        result: value,
        value: root,
    });
    let ok = construct(
        &mut b,
        result,
        ConstructorKind::Variant {
            definition,
            variant: 0,
        },
        &[value],
    )?;
    b.ret(Some(ok));
    b.at(kept);
    let error = construct(
        &mut b,
        result,
        ConstructorKind::Variant {
            definition,
            variant: 1,
        },
        &[root],
    )?;
    b.ret(Some(error));
    Ok(b.finish())
}
