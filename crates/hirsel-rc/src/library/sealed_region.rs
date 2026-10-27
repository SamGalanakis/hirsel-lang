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
    let receipt = if descriptor.bits_root(cx.layouts.table()).is_err() {
        let contract = cx.fn_abi().receipts.ok_or_else(|| {
            PassError::Unsupported(
                "sealed payload needs complete original-source receipt transport".into(),
            )
        })?;
        if !matches!(contract.parameters.as_slice(), [parameter]
            if parameter.source == 1 && parameter.mode == hirsel_lir::sealed_payload_source::ReceiptMode::Take)
            || contract.result.is_some()
            || !contract.captures.is_empty()
            || !contract.inventory.is_empty()
        {
            return cx.unsupported("seal requires exactly its paired Take root receipt");
        }
        true
    } else {
        false
    };
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
    let failure = if b.owns(b.layout_of(root)) {
        let at = b.current();
        let held = b.held_state();
        let failed = b.block(BlockKind::FailureCleanup, &[]);
        b.at(failed);
        // The runtime rolls back private staging. The entered body still
        // owns the extracted root and its companion on a failed transaction.
        let (slot, address) = super::iter::local(&mut b, root)?;
        super::iter::drop_local(&mut b, slot, address);
        for (slot, address) in held.iter().copied().rev() {
            super::iter::drop_local(&mut b, slot, address);
        }
        b.terminate(Terminator::Propagate);
        b.restore_held(&held);
        b.at(at);
        b.failure_to(failed, &[], None)
    } else {
        b.failure(None)
    };
    b.push(Inst::SealArena {
        result,
        grant,
        root,
        shape: SealDescriptorId(0),
        failure,
        origin: b.anchor,
    });
    let result = if receipt {
        // Keep the new owner in cleanup custody until metadata retirement has
        // finished on both outcomes. The seal helper borrows the entry snapshot.
        let (owner_slot, owner_address) = super::iter::local(&mut b, result)?;
        let slot = b.ed.unit.tables.receipt_entry.as_ref().unwrap().parameters[0].slot;
        let address =
            b.ed.unit
                .addresses
                .iter()
                .position(|a| a.base == AddressBase::Slot(slot) && a.projection.len == 0)
                .unwrap();
        super::iter::drop_local(&mut b, slot, AddressId(address as u32));
        let result = b.take(owner_address)?;
        b.push(Inst::StorageDead {
            slot: owner_slot,
            if_live: false,
        });
        result
    } else {
        result
    };
    b.ret(Some(result));
    Ok(b.finish())
}

/// Reconstruct the seal entry's data contract from the current reviewed core
/// declaration and complete original root actual, before building native IR.
/// This supplies a declaration; C still proves live same-snapshot custody.
pub fn receipt_source(
    program: &crate::instance::Program,
    root: hirsel_types::TypeId,
    key: &hirsel_layout::instance::InstanceKey,
) -> Result<hirsel_lir::sealed_payload_source::ReceiptSource, PassError> {
    use hirsel_lir::sealed_payload_source::*;
    use hirsel_syntax::{ContentHash, DefPath};
    let path = DefPath::core("arena.seal");
    let signature = program.signatures.get(&path).ok_or_else(|| {
        PassError::Malformed("seal receipt lacks its reviewed declaration".into())
    })?;
    if program.operations.get(&path).is_none_or(|operation| {
        operation.definition != path || operation.declaration_digest != signature.fingerprint
    }) {
        return Err(PassError::Malformed(
            "seal receipt declaration differs from its reviewed operation".into(),
        ));
    }
    let request = super::Request::operation(
        program,
        usize::MAX,
        &path,
        vec![GenericArgument::Type(root)],
    )?;
    // The ordinary key contains complete original actuals. No layout or
    // candidate header can choose another root for the independent declaration.
    let expected = hirsel_layout::instance::InstanceKey::new(
        &path,
        0,
        &request.actuals.key_binders(),
        &program.types,
        None,
        None,
    )
    .map_err(|error| crate::instance::InstanceError::Key(path.clone(), error))?;
    if key != &expected {
        return Err(PassError::Malformed(
            "seal receipt differs from its original typed instance".into(),
        ));
    }
    let profile = crate::sealed_source::profile::derive(
        &program.types,
        root,
        &mut |definition| Ok(program.signatures.get(definition.path()).cloned()),
        &mut |_| {
            Err(crate::sealed_source::SourceError::Unsupported(
                "seal receipt requires independently selected held code",
            ))
        },
    )
    .map_err(|error| PassError::Unsupported(format!("seal original root profile: {error:?}")))?;
    let profile_identity = profile
        .content()
        .map_err(|error| PassError::Malformed(error.into()))?;
    let contract = ReceiptContract::new(
        vec![ReceiptParameter {
            source: 1,
            ty: ReceiptType {
                source_type: profile.root.source_type,
                profile: profile_identity,
            },
            mode: ReceiptMode::Take,
        }],
        None,
        vec![],
        vec![],
    )
    .map_err(|error| PassError::Malformed(error.into()))?;
    let source = ReceiptSource {
        original: hirsel_lir::invocations::ReferenceBody {
            definition: path.clone(),
            instance: key.digest(),
            body: 0,
        },
        instance: key.digest(),
        source: ContentHash::of_parts(
            "hirsel.reviewed-seal-entry.v1",
            [
                signature.fingerprint.0.as_slice(),
                profile_identity.0.as_slice(),
            ],
        ),
        implementation_revision: super::revision(&path),
        contract,
        profiles: vec![ReceiptProfile {
            port: ReceiptPort::Parameter(1),
            profile,
        }],
    };
    source
        .check_structure()
        .map_err(|error| PassError::Malformed(error.into()))?;
    Ok(source)
}
