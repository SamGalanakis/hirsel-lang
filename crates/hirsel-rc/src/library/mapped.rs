//! `mapped.with[R](d, m, body) -> Result[R, FileError]` over the runtime's mapping entries
//! (`hirsel_rt_v2_mapped_view`, `hirsel_rt_v2_mapped_check`, `hirsel-rt/src/mapping.rs`).
//! The body borrows the view `$intrinsic.MappedView` returns, which is released after it;
//! then `$intrinsic.MappedCheck` says whether the file was truncated meanwhile, and the
//! body's result is dropped for `Error(Truncated())`.
use super::LibraryBody;
use super::arena::{RESULT_ERROR, RESULT_OK, payload, variant};
use super::builder::{Body, Cx};
use super::iter::{drop_local, local};
use super::tasks::{argument, intrinsic, invoke, operation, parameter};
use crate::PassError;
use hirsel_core_contract::Passing;
use hirsel_lir::abi_types::CallableShape;
use hirsel_lir::ir::*;
use hirsel_lir::stage::Value;
use hirsel_syntax::DefPath;

type R<T> = Result<T, PassError>;

pub static WITH: LibraryBody = LibraryBody {
    path: || DefPath::core("mapped.with"),
    build: with,
};

/// `FileError::Truncated()`'s ordinal.
const TRUNCATED: u32 = 10;

fn with(cx: &mut Cx<'_>) -> R<LirUnit<Value>> {
    let view_entry = intrinsic(cx, "MappedView", Vec::new())?;
    let check_entry = intrinsic(cx, "MappedCheck", Vec::new())?;
    let callback = parameter(&cx.store, cx.callable, 2)?;
    let abi = cx
        .layouts
        .fn_abi(&cx.store, callback, CallableShape::Closure)
        .map_err(|e| PassError::Layout(cx.path.clone(), e))?;
    let returned = cx
        .fn_abi()
        .result
        .map(|r| r.layout)
        .ok_or_else(|| PassError::Malformed("mapped.with returns a Result".into()))?;
    let mut b = Body::new(cx)?;
    let &[d, m, body] = b.parameters.as_slice() else {
        return Err(PassError::Malformed("mapped.with parameters".into()));
    };
    let error = payload(&b, returned, RESULT_ERROR)
        .ok_or_else(|| PassError::Malformed("mapped.with's Result has no error".into()))?;
    let handles = |b: &mut Body<'_, '_>| -> R<_> {
        let (directory, directory_loan) = b.borrow(d.address, false)?;
        let (mapping, mapping_loan) = b.borrow(m.address, false)?;
        Ok((
            vec![
                argument(0, Passing::Borrow, directory),
                argument(1, Passing::Borrow, mapping),
            ],
            [directory_loan, mapping_loan],
        ))
    };
    let (arguments, loans) = handles(&mut b)?;
    let failure = b.failure(None);
    let callee = operation(&mut b, &view_entry);
    let view = invoke(
        &mut b,
        callee,
        view_entry.abi,
        arguments,
        &loans,
        failure,
        BlockKind::Body,
    )
    .ok_or_else(|| PassError::Malformed("MappedView returns a view".into()))?;
    let (slot, address) = local(&mut b, view)?;
    let (view, view_loan) = b.borrow(address, false)?;
    let (callee, loan) = b.borrow(body.address, false)?;
    let result = super::iter::call(
        &mut b,
        callee,
        abi,
        vec![argument(0, Passing::Borrow, view)],
        &[view_loan, loan],
    )?;
    // The result waits in a local, so a failing view release or check releases it.
    let result = result.map(|value| local(&mut b, value)).transpose()?;
    drop_local(&mut b, slot, address);
    let (arguments, loans) = handles(&mut b)?;
    let failure = b.failure(None);
    let callee = operation(&mut b, &check_entry);
    let status = invoke(
        &mut b,
        callee,
        check_entry.abi,
        arguments,
        &loans,
        failure,
        BlockKind::Body,
    )
    .ok_or_else(|| PassError::Malformed("MappedCheck returns a status".into()))?;
    let at = b.current();
    let whole = b.block(BlockKind::Body, &[]);
    b.at(whole);
    let ok = match result {
        Some((slot, address)) => {
            let value = b.take(address)?;
            b.emptied(slot);
            b.push(Inst::StorageDead {
                slot,
                if_live: false,
            });
            vec![value]
        }
        None => match payload(&b, returned, RESULT_OK) {
            Some(unit) => vec![b.constant(unit, ConstantValue::Unit)],
            None => Vec::new(),
        },
    };
    let value = variant(&mut b, returned, "Result", RESULT_OK, &ok);
    b.ret(Some(value));
    let truncated = b.block(BlockKind::Body, &[]);
    b.at(truncated);
    if let Some((slot, address)) = result {
        drop_local(&mut b, slot, address);
    }
    let cause = variant(&mut b, error, "FileError", TRUNCATED, &[]);
    let value = variant(&mut b, returned, "Result", RESULT_ERROR, &[cause]);
    b.ret(Some(value));
    let broken = b.block(BlockKind::Body, &[]);
    b.at(broken);
    b.terminate(Terminator::InvariantFault {
        reason: "a mapping check status past Truncated".into(),
    });
    b.at(at);
    let arms = vec![
        SwitchArm {
            value: 0,
            edge: b.edge(whole, &[]),
        },
        SwitchArm {
            value: 1,
            edge: b.edge(truncated, &[]),
        },
    ];
    let otherwise = b.edge(broken, &[]);
    let arms = PoolRange::push(&mut b.ed.unit.pools.switch_arms, arms);
    b.terminate(Terminator::Switch {
        value: status,
        arms,
        otherwise,
    });
    Ok(b.finish())
}
