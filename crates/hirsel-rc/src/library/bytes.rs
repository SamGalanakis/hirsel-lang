//! Byte-list freezing and checked forward history appends.
use super::{CallShape, InlineLowering, R};
use crate::import::ImportCx;
use hirsel_lir::ir::{ConstantValue, ConstructorKind, Holding, Inst};
use hirsel_lir::layout::sequence_slice_start;
use hirsel_lir::layout_types::LayoutKind;
use hirsel_types::UseKind;

pub static FROM_LIST: InlineLowering = InlineLowering {
    path: || hirsel_syntax::DefPath::core("bytes.from_list"),
    lower: from_list,
};

// Same owned sequence/slice representation as bytes.from_list. The checker
// selects this only for compiler-created primitive-only f-strings.
pub static TEXT_FINISH: InlineLowering = InlineLowering {
    path: || hirsel_syntax::DefPath::core("$intrinsic.TextFinish"),
    lower: from_list,
};

fn from_list(cx: &mut ImportCx<'_>, call: &CallShape<'_>) -> R<bool> {
    let [argument] = call.arguments else {
        return Ok(false);
    };
    if argument.value.usage != UseKind::Take {
        return Ok(false);
    }
    let owner = cx.operand(&argument.value)?;
    let layout = cx.place_layout(call.destination)?;
    let word = cx.record(layout).fields[1].layout;
    let length = cx.value(word, Holding::Plain);
    cx.push(Inst::SeqLen {
        result: length,
        value: owner,
    });
    // The whole sequence: its first element past the slice data offset.
    let element = match cx.record(layout).kind {
        LayoutKind::Slice { element } => element,
        _ => return Ok(false),
    };
    let start = sequence_slice_start(cx.record(element))
        .map_err(|e| crate::PassError::Malformed(format!("{e:?}")))?;
    let start = cx.constant(word, ConstantValue::Integer(u128::from(start)));
    let result = cx.construct(
        layout,
        layout,
        ConstructorKind::Tuple,
        &[owner, start, length],
    )?;
    cx.finish(call, result)?;
    Ok(true)
}

// This operation has one compiled wrapper for direct calls and function values.
// Validate before charging an arena, then call the same runtime leaf on both
// backends. No source mutable alias of the history is ever constructed.
pub static APPEND_FROM_HISTORY: super::LibraryBody = super::LibraryBody {
    path: || hirsel_syntax::DefPath::core("bytes.append_from_history"),
    build: super::value::forward,
};

pub(super) fn prepare_history(b: &mut super::builder::Body<'_, '_>) -> R<()> {
    use hirsel_lir::ir::*;
    let list = b.parameters[0];
    let distance = b.read_bits(b.parameters[1].address)?;
    let count = b.read_bits(b.parameters[2].address)?;
    let (view, loan) = b.borrow(list.address, false)?;
    let len = b.seq_len(view)?;
    b.end_loan(loan);
    let zero = b.u64(0)?;
    let positive = b.compare(BinaryOp::Gt, distance, zero)?;
    let failure = b.failure(Some("SEM-3"));
    b.push(Inst::Check {
        condition: positive,
        failure,
    });
    let within = b.compare(BinaryOp::Le, distance, len)?;
    let failure = b.failure(Some("SEM-3"));
    b.push(Inst::Check {
        condition: within,
        failure,
    });
    // A sequence extent must fit its eight-byte prefix and capacity word too.
    let maximum = b.u64(hirsel_lir::runtime_abi::EXTENT_MAX - 16)?;
    let valid_len = b.compare(BinaryOp::Le, len, maximum)?;
    let failure = b.failure(Some("SEM-3"));
    b.push(Inst::Check {
        condition: valid_len,
        failure,
    });
    let room = b.value(b.layout_of(len), Holding::Plain);
    let failure = b.failure(Some("SEM-1"));
    b.push(Inst::Binary {
        result: room,
        op: BinaryOp::Sub,
        left: maximum,
        right: len,
        failure,
    });
    let fits = b.compare(BinaryOp::Le, count, room)?;
    let failure = b.failure(Some("SEM-3"));
    b.push(Inst::Check {
        condition: fits,
        failure,
    });
    let done = b.block(BlockKind::Body, &[]);
    let append = b.block(BlockKind::Body, &[]);
    let empty = b.compare(BinaryOp::Eq, count, zero)?;
    b.branch(empty, done, append);
    b.at(done);
    let unit =
        b.cx.fn_abi()
            .result
            .map(|r| b.constant(r.layout, ConstantValue::Unit));
    b.ret(unit);
    b.at(append);
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
    Ok(())
}

/// Direct calls and function values share validation and branch-local COW.
pub static REPLACE_RANGE: super::LibraryBody = super::LibraryBody {
    path: || hirsel_syntax::DefPath::core("bytes.replace_range"),
    build: super::value::forward,
};

pub(super) fn prepare_replace_range(b: &mut super::builder::Body<'_, '_>) -> R<()> {
    use hirsel_lir::ir::*;
    let list = b.parameters[0];
    let start = b.read_bits(b.parameters[1].address)?;
    let (view, loan) = b.borrow(list.address, false)?;
    let len = b.seq_len(view)?;
    b.end_loan(loan);
    let (source, loan) = b.borrow(b.parameters[2].address, false)?;
    let count = b.seq_len(source)?;
    b.end_loan(loan);
    let within = b.compare(BinaryOp::Le, start, len)?;
    let failure = b.failure(Some("MEM-14"));
    b.push(Inst::Check {
        condition: within,
        failure,
    });
    let room = b.value(b.layout_of(len), Holding::Plain);
    let failure = b.failure(Some("MEM-14"));
    b.push(Inst::Binary {
        result: room,
        op: BinaryOp::Sub,
        left: len,
        right: start,
        failure,
    });
    let fits = b.compare(BinaryOp::Le, count, room)?;
    let failure = b.failure(Some("MEM-14"));
    b.push(Inst::Check {
        condition: fits,
        failure,
    });
    let zero = b.u64(0)?;
    let empty = b.compare(BinaryOp::Eq, count, zero)?;
    let done = b.block(BlockKind::Body, &[]);
    let write = b.block(BlockKind::Body, &[]);
    b.branch(empty, done, write);
    b.at(done);
    let unit =
        b.cx.fn_abi()
            .result
            .map(|r| b.constant(r.layout, ConstantValue::Unit));
    b.ret(unit);
    b.at(write);
    // P4 recognizes only the leaf's held-list write. Its address must be
    // prepared here so COW cannot migrate above validation or the empty branch.
    b.parameters[0].address = b.place(AddressBase::Slot(list.slot), &[], list.layout, None)?;
    Ok(())
}
