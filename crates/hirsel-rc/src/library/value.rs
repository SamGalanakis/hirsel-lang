//! An operation used as a function value (`let f = out.print`): a registry
//! operation without a library body has no code a `FunctionRef` can name, so
//! its value names this body, instantiated at the operation's own path and
//! actuals. The body passes each parameter on as declared and calls the
//! operation, so the operation keeps its one native realisation and the call
//! through the value fails, cleans up and returns exactly as a direct call.
//! Of the operations lowered inline at a call, only the lengths have a value
//! here, the `SeqLen` of their borrowed argument (`length.rs`), and String to
//! Bytes copies the same Slice representation; the others
//! are unsupported as values.
use super::builder::{Body, Cx};
use super::{LibraryBody, native_path};
use crate::PassError;
use hirsel_core_contract::Passing;
use hirsel_lir::abi_types::CallableShape;
use hirsel_lir::ir::*;
use hirsel_lir::stage::Value;
use hirsel_syntax::DefPath;

type R<T> = Result<T, PassError>;

/// Registered under its own helper path; each instance is keyed by the
/// operation it forwards to (`Request::operation_value`).
pub static FORWARD: LibraryBody = LibraryBody {
    path: || native_path("operation_value"),
    build: forward,
};

/// The inline lowerings whose value this body realises.
const LENGTHS: [&str; 4] = ["list.len", "array.len", "string.byte_len", "bytes.len"];

fn length(path: &DefPath) -> bool {
    LENGTHS
        .iter()
        .any(|name| hirsel_syntax::DefPath::core(name) == *path)
}

/// Whether the value of operation `path`, which has no library body, is this
/// body: every operation a call realises, and the lengths.
pub fn forwards(path: &DefPath) -> bool {
    super::inline_lowering(path).is_none() || length(path) || path.is_core("string.as_bytes")
}

pub(super) fn forward(cx: &mut Cx<'_>) -> R<LirUnit<Value>> {
    let path = cx.path.clone();
    if path.is_core("string.as_bytes") {
        let mut b = Body::new(cx)?;
        let source = b.parameters[0];
        let failure = b.failure(None);
        let result = b.copy(source.address, failure)?;
        b.ret(Some(result));
        return Ok(b.finish());
    }
    if length(&path) {
        let mut b = Body::new(cx)?;
        let &[sequence] = b.parameters.as_slice() else {
            return Err(PassError::Malformed(format!("{path:?} parameters")));
        };
        let (value, loan) = b.borrow(sequence.address, false)?;
        let result = b.seq_len(value)?;
        b.end_loan(loan);
        b.ret(Some(result));
        return Ok(b.finish());
    }
    let operation = cx
        .program
        .operations
        .get(&path)
        .cloned()
        .ok_or_else(|| PassError::Malformed(format!("{path:?} is not an operation")))?;
    let operation_arguments = hirsel_layout::operation::collect_arguments(cx.types, &cx.arguments)
        .map_err(|e| PassError::Layout(path.clone(), e))?;
    // The forwarding body carries caller metadata independently of the primitive's
    // declared physical ABI. Only primitives that explicitly need it receive it.
    let abi = cx
        .layouts
        .fn_abi(cx.types, cx.callable, CallableShape::Definition)
        .map_err(|e| PassError::Layout(path.clone(), e))?;
    let abi = if cx
        .program
        .signatures
        .get(&path)
        .is_some_and(|s| s.metadata.track_caller)
    {
        cx.layouts
            .with_caller_site(abi)
            .map_err(|e| PassError::Layout(path.clone(), e))?
    } else {
        abi
    };
    let sources: Vec<u32> = cx.fn_abi().parameters.iter().map(|p| p.source).collect();
    let mut b = Body::new(cx)?;
    if path.is_core("bytes.append_from_history") {
        super::bytes::prepare_history(&mut b)?;
    }
    if path.is_core("bytes.append") {
        let list = b.parameters[0];
        let data = b.parameters[1];
        let (view, loan) = b.borrow(data.address, false)?;
        let count = b.seq_len(view)?;
        b.end_loan(loan);
        let zero = b.u64(0)?;
        let empty = b.compare(BinaryOp::Eq, count, zero)?;
        let done = b.block(BlockKind::Body, &[]);
        let append = b.block(BlockKind::Body, &[]);
        b.branch(empty, done, append);
        b.at(done);
        let unit =
            b.cx.fn_abi()
                .result
                .map(|r| b.constant(r.layout, ConstantValue::Unit));
        b.ret(unit);
        b.at(append);
        // Keep arena preparation on this branch, separate from the parameter
        // address lent to the operation. Reusing that entry address makes P4
        // detach its held block at entry, even for an empty append.
        let address = b.place(AddressBase::Slot(list.slot), &[], list.layout, None)?;
        let version = b.version(list.slot);
        let failure = b.failure(None);
        b.push(Inst::ArenaPush {
            address,
            count: Some(count),
            version,
            failure,
        });
        b.set_reaching(list.slot, version);
    }
    if path.is_core("bytes.replace_range") {
        super::bytes::prepare_replace_range(&mut b)?;
    }
    let mut arguments = Vec::new();
    let mut loans = Vec::new();
    for (p, parameter) in b.parameters.clone().into_iter().zip(sources) {
        let value = match p.passing {
            Passing::Take => ArgumentValue::Value(b.take(p.address)?),
            Passing::Borrow => {
                let (value, loan) = b.borrow(p.address, false)?;
                loans.push(loan);
                ArgumentValue::Value(value)
            }
            Passing::Mut => {
                let version = b.version(p.slot);
                b.set_reaching(p.slot, version);
                ArgumentValue::Address {
                    address: p.address,
                    version,
                }
            }
        };
        arguments.push(CallArgument {
            parameter,
            passing: p.passing,
            value,
        });
    }
    let reference = OperationRef(b.ed.unit.tables.operations.len() as u32);
    b.ed.unit.tables.operations.push(operation);
    let result = b.call(
        Callee::Operation(reference),
        operation_arguments,
        abi,
        arguments,
        &loans,
    );
    b.ret(result);
    Ok(b.finish())
}
