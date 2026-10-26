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
    let abi = cx
        .layouts
        .with_scoped_contract(
            abi,
            vec![hirsel_lir::abi_types::ParameterPath {
                parameter: 0,
                fields: Vec::new(),
            }],
            Vec::new(),
        )
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

/// The entered seal body consumes its two declared Take parameters. This is
/// unregistered until semantic inventory transport and both emitters are linked.
pub static SEAL: LibraryBody = LibraryBody {
    path: || hirsel_syntax::DefPath::core("arena.seal"),
    build: seal,
};

fn seal(cx: &mut Cx<'_>) -> Result<LirUnit<Value>, PassError> {
    let root = cx.type_actual(0)?;
    let native =
        hirsel_layout::operation::collect_arguments(cx.types, &[GenericArgument::Type(root)])
            .map_err(|e| PassError::Layout(cx.path.clone(), e))?;
    let [OperationArgument::Type(native)] = native.as_slice() else {
        return Err(PassError::Malformed("arena.seal root actual".into()));
    };
    let descriptor = cx
        .layouts
        .seal_descriptor(&cx.program.operation_registry(), native)
        .map_err(|e| PassError::Layout(cx.path.clone(), e))?;
    if !matches!(
        cx.record(descriptor.root_layout).kind,
        LayoutKind::Unit | LayoutKind::Scalar(_)
    ) {
        return cx.unsupported("sealed payload needs complete original-source receipt transport");
    }
    let owner_layout = cx
        .fn_abi()
        .result
        .ok_or_else(|| PassError::Malformed("arena.seal returns an owner".into()))?
        .layout;
    let mut b = Body::new(cx)?;
    let [grant, root] = b.parameters[..] else {
        return Err(PassError::Malformed("arena.seal parameters".into()));
    };
    if grant.passing != Passing::Take
        || root.passing != Passing::Take
        || b.record(grant.layout).kind != LayoutKind::Arena
        || root.layout != descriptor.root_layout
        || !matches!(b.record(owner_layout).kind,
            LayoutKind::SealedRegion { root } if root == descriptor.root_layout)
    {
        return Err(PassError::Malformed(
            "arena.seal take parameters or owner layout".into(),
        ));
    }
    b.ed.unit.tables.seal_descriptors.push(descriptor);
    let grant = b.take(grant.address)?;
    let root = b.take(root.address)?;
    let result = b.fresh(owner_layout);
    let failure = b.failure(None);
    b.push(Inst::SealArena {
        result,
        grant,
        root,
        shape: SealDescriptorId(0),
        failure,
        origin: b.anchor,
    });
    b.ret(Some(result));
    Ok(b.finish())
}
