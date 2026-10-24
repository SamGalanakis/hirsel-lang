//! `net.read_into` and `read_half.read_into`. A runtime symbol cannot
//! name the size `k`, so one body per `k` calls `$intrinsic.NetReadInto[k]` (or
//! `$intrinsic.ReadHalfReadInto[k]`), the fixed runtime entry `hirsel_rt_v2_net_read_into`,
//! with the buffer's address and `k`.

use super::LibraryBody;
use super::builder::{Body, Cx};
use crate::PassError;
use crate::instance::{Actuals, InstanceError, instantiate_function};
use hirsel_core_contract::Passing;
use hirsel_lir::abi_types::CallableShape;
use hirsel_lir::ir::*;
use hirsel_lir::stage::Value;
use hirsel_types::SignatureKind;

type R<T> = Result<T, PassError>;

pub static READ_INTO: LibraryBody = LibraryBody {
    path: || hirsel_syntax::DefPath::core("net.read_into"),
    build: |cx| read_into(cx, "$intrinsic.NetReadInto"),
};

pub static HALF_READ_INTO: LibraryBody = LibraryBody {
    path: || hirsel_syntax::DefPath::core("read_half.read_into"),
    build: |cx| read_into(cx, "$intrinsic.ReadHalfReadInto"),
};

fn read_into(cx: &mut Cx<'_>, intrinsic: &str) -> R<LirUnit<Value>> {
    let k = cx.size_actual(0)?;
    let path = hirsel_syntax::DefPath::core(intrinsic);
    let operation = cx
        .program
        .operations
        .get(&path)
        .cloned()
        .ok_or_else(|| PassError::Malformed(format!("{path:?} is not an operation")))?;
    // The intrinsic's `FnAbi` under this body's own actuals, as a call site computes it.
    let signature = cx
        .program
        .signatures
        .get(&path)
        .ok_or_else(|| InstanceError::MissingSignature(path.clone()))?
        .clone();
    let SignatureKind::Function(function) = &signature.kind else {
        return Err(PassError::Malformed(format!("{path:?} is not a function")));
    };
    let actuals = Actuals {
        binder: Some(super::binder(&path)),
        arguments: cx.arguments.clone(),
        outer: Vec::new(),
    };
    let callable = instantiate_function(
        &cx.program.types,
        &cx.program.memo,
        function,
        hirsel_types::Modes::global(),
        &actuals,
    )
    .map_err(|e| InstanceError::Substitution(path.clone(), e))?;
    let abi = cx
        .layouts
        .fn_abi(cx.types, callable, CallableShape::Definition)
        .map_err(|e| PassError::Layout(path.clone(), e))?;
    let mut b = Body::new(cx)?;
    let &[n, s, buffer] = b.parameters.as_slice() else {
        return Err(PassError::Malformed("net.read_into parameters".into()));
    };
    let capacity = b.u64(k)?;
    let (net, loan) = b.borrow(n.address, false)?;
    let exclusive = |b: &mut Body<'_, '_>, parameter: u32, address: AddressId| {
        let slot = b.root(address).expect("mut parameter place");
        let version = b.version(slot);
        b.set_reaching(slot, version);
        CallArgument {
            parameter,
            passing: Passing::Mut,
            value: ArgumentValue::Address { address, version },
        }
    };
    let arguments = [
        CallArgument {
            parameter: 0,
            passing: Passing::Borrow,
            value: ArgumentValue::Value(net),
        },
        exclusive(&mut b, 1, s.address),
        exclusive(&mut b, 2, buffer.address),
        CallArgument {
            parameter: 3,
            passing: Passing::Borrow,
            value: ArgumentValue::Value(capacity),
        },
    ];
    let fn_abi = b.cx.layouts.table().fn_abis[abi.0 as usize].clone();
    let result = fn_abi.result.map(|r| b.fresh(r.layout));
    let reference = OperationRef(b.ed.unit.tables.operations.len() as u32);
    b.ed.unit.tables.operations.push(operation);
    let arguments = PoolRange::push(&mut b.ed.unit.pools.call_arguments, arguments);
    let call = CallId(b.ed.unit.tables.calls.len() as u32);
    b.ed.unit.tables.calls.push(CallSite {
        receipts: None,
        pretransfer_failure: None,
        forward_caller: false,
        operation_arguments: vec![hirsel_lir::operation::OperationArgument::Size(k)],
        callee: Callee::Operation(reference),
        abi,
        arguments,
        result,
        destination: fn_abi.destination.then_some(DestinationClass::Inferred),
        declared: crate::import::CALL_EFFECTS,
        anchor: b.anchor,
    });
    // A failed call ends the loan on `n` before the body's own cleanup.
    let next = b.failure(None);
    let cleanup = b.ed.unit.tables.failures[next.index()].cleanup;
    let ending = b.block(BlockKind::FailureCleanup, &[]);
    b.ed.code[ending.index()].push(Inst::EndLoan {
        loan,
        if_active: false,
    });
    b.ed.unit.terminators[ending.index()] = Terminator::Jump(cleanup);
    let failure = b.failure_to(ending, &[], None);
    let after = b.block(BlockKind::Body, &[]);
    let normal = b.edge(after, &[]);
    b.terminate(Terminator::Call {
        call,
        normal: Some(normal),
        failure,
    });
    b.at(after);
    b.end_loan(loan);
    b.ret(result);
    Ok(b.finish())
}
