//! The read-only sealed-root library body. This remains outside BODIES until
//! independent source declarations, H and both emitters link the full pipeline.
use super::LibraryBody;
use super::builder::{Body, Cx};
use crate::PassError;
use hirsel_core_contract::Passing;
use hirsel_lir::abi_types::CallableShape;
use hirsel_lir::ir::*;
use hirsel_lir::layout_types::LayoutKind;
use hirsel_lir::operation::OperationArgument;
use hirsel_lir::stage::Value;
use hirsel_types::GenericArgument;

pub static WITH: LibraryBody = LibraryBody {
    path: || hirsel_syntax::DefPath::core("region.with"),
    build: with,
};

fn with(cx: &mut Cx<'_>) -> Result<LirUnit<Value>, PassError> {
    let root = cx.type_actual(0)?;
    let callback = super::tasks::parameter(cx.types, cx.callable, 1)?;
    let abi = cx
        .layouts
        .fn_abi(cx.types, callback, CallableShape::Closure)
        .map_err(|e| PassError::Layout(cx.path.clone(), e))?;
    let native =
        hirsel_layout::operation::collect_arguments(cx.types, &[GenericArgument::Type(root)])
            .map_err(|e| PassError::Layout(cx.path.clone(), e))?;
    let [OperationArgument::Type(native)] = native.as_slice() else {
        return Err(PassError::Malformed("region.with root actual".into()));
    };
    let descriptor = cx
        .layouts
        .seal_descriptor(&cx.program.operation_registry(), native)
        .map_err(|e| PassError::Layout(cx.path.clone(), e))?;
    if !matches!(
        cx.record(descriptor.root_layout).kind,
        LayoutKind::Unit | LayoutKind::Scalar(_)
    ) {
        return cx.unsupported(
            "sealed payload callback needs an independently qualified data-root entry",
        );
    }
    let mut b = Body::new(cx)?;
    let [owner, callback] = b.parameters[..] else {
        return Err(PassError::Malformed("region.with parameters".into()));
    };
    if owner.passing != Passing::Mut
        || !matches!(b.record(owner.layout).kind,
            LayoutKind::SealedRegion { root } if root == descriptor.root_layout)
    {
        return Err(PassError::Malformed(
            "region.with requires its mut sealed owner".into(),
        ));
    }
    let root_layout = descriptor.root_layout;
    b.ed.unit.tables.seal_descriptors.push(descriptor);
    let (owner, owner_loan) = b.borrow(owner.address, true)?;
    let holding = if b.owns(root_layout) {
        Holding::Borrowed(owner_loan)
    } else {
        Holding::Plain
    };
    let view = b.value(root_layout, holding);
    b.push(Inst::BorrowSealedRoot {
        result: view,
        owner,
        loan: owner_loan,
        shape: SealDescriptorId(0),
    });
    let (callee, callback_loan) = b.borrow(callback.address, false)?;
    let result = super::iter::call(
        &mut b,
        callee,
        abi,
        vec![CallArgument {
            parameter: 0,
            passing: Passing::Borrow,
            value: ArgumentValue::Value(view),
        }],
        &[owner_loan, callback_loan],
    )?;
    b.ret(result);
    Ok(b.finish())
}
