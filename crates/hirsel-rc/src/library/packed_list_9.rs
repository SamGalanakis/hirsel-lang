//! MEM-13 truncation using existing sequential walks and appends.
//!
//! Rust Vec::truncate retains a prefix before retiring the tail. Packed values
//! are flat and have no finalizers; copy the prefix before replacing its owner.
//! This fallback deliberately gives up unique in-place shrink/capacity retention.
use super::builder::{Body, Cx};
use crate::PassError;
use hirsel_lir::ir::*;
use hirsel_lir::layout_types::{LayoutId, LayoutKind};
use hirsel_lir::stage::Value;

fn empty(b: &mut Body<'_, '_>, layout: LayoutId, payload: LayoutId) -> ValueId {
    let instance = hirsel_lir::LayoutKeys::rooted(b.cx.layouts.table(), payload).0;
    let symbol = b.symbol(Symbol {
        kind: SymbolKind::Constant,
        definition: crate::import::empty_sequence(),
        instance: Some(instance),
    });
    let result = b.fresh(layout);
    b.push(Inst::ReadStatic { result, symbol });
    result
}

fn ret_unit(b: &mut Body<'_, '_>) {
    let result =
        b.cx.fn_abi()
            .result
            .map(|r| b.constant(r.layout, ConstantValue::Unit));
    b.ret(result);
}

/// A separate body keeps Packed routing out of the fixed-stride tail walk.
pub(super) fn truncate(cx: &mut Cx<'_>) -> Result<LirUnit<Value>, PassError> {
    let mut b = Body::new(cx)?;
    let list = b.parameters[0];
    let count = b.read_bits(b.parameters[1].address)?;
    let payload =
        b.cx.payload(list.layout)
            .ok_or_else(|| PassError::Malformed("Packed list payload".into()))?;
    let LayoutKind::Sequential { element } = b.record(payload).kind else {
        return Err(PassError::Malformed(
            "Packed truncate of fixed-stride list".into(),
        ));
    };
    let (source, loan) = b.borrow(list.address, false)?;
    let first_failure = b.ed.unit.tables.failures.len();
    let len = b.seq_len(source)?;
    let shorter = b.compare(BinaryOp::Lt, count, len)?;
    let rebuild = b.block(BlockKind::Body, &[]);
    let noop = b.block(BlockKind::Body, &[]);
    b.branch(shorter, rebuild, noop);
    b.at(noop);
    b.end_loan(loan);
    ret_unit(&mut b);
    b.at(rebuild);
    let zero = b.u64(0)?;
    let one = b.u64(1)?;
    let word = b.u64_layout()?;
    let index = b.value(word, Holding::Plain);
    let offset = b.value(word, Holding::Plain);
    let measure = b.block(BlockKind::Body, &[index, offset]);
    let step = b.block(BlockKind::Body, &[]);
    let measured = b.block(BlockKind::Body, &[]);
    b.jump(measure, &[zero, zero]);
    b.at(measure);
    let more = b.compare(BinaryOp::Lt, index, count)?;
    b.branch(more, step, measured);
    b.at(step);
    let at = b.place(
        AddressBase::Value(source),
        &[ProjectionStep::FlatAt { index, offset }],
        element,
        Some("MEM-14"),
    )?;
    let end = b.value(word, Holding::Plain);
    b.push(Inst::FlatNext {
        result: end,
        address: at,
    });
    let next = advance(&mut b, index, one);
    b.jump(measure, &[next, end]);
    b.at(measured);
    let output = b.slot(list.layout, SlotKind::Temporary);
    b.push(Inst::StorageLive { slot: output });
    let initial = b.place(AddressBase::Slot(output), &[], list.layout, None)?;
    let vacant = empty(&mut b, list.layout, payload);
    b.store(initial, vacant)?;
    b.hold(output, initial);
    let nonempty = b.compare(BinaryOp::Ne, count, zero)?;
    let reserve = b.block(BlockKind::Body, &[]);
    let output_version = b.version(output);
    let complete = b.block(BlockKind::Body, &[output_version]);
    let before = b.reaching(output)?;
    let yes = b.edge(reserve, &[]);
    let no = b.edge(complete, &[before]);
    b.terminate(Terminator::Branch {
        condition: nonempty,
        yes,
        no,
    });
    b.at(reserve);
    // SeqReserve counts least-sized elements. Round the measured byte extent
    // up to that quantum; EXTENT_MAX leaves room for this small addition.
    let least = hirsel_lir::layout::least_size(&b.record(element).size);
    if least == 0 {
        return Err(PassError::Malformed(
            "Packed element has no tag extent".into(),
        ));
    }
    let rounding = b.u64(least - 1)?;
    let rounded = b.add(ArithMode::Wrapping, offset, rounding);
    let quantum = b.u64(least)?;
    let capacity = b.value(word, Holding::Plain);
    let failure = b.failure(None);
    b.push(Inst::Binary {
        result: capacity,
        op: BinaryOp::Div,
        left: rounded,
        right: quantum,
        failure,
    });
    // A distinct address keeps initialization separate from reservation. The
    // reservation itself detaches static empty storage once at its final size.
    let destination = b.place(AddressBase::Slot(output), &[], list.layout, None)?;
    b.seq_reserve(destination, capacity)?;
    let i = b.value(word, Holding::Plain);
    let at_offset = b.value(word, Holding::Plain);
    let version = b.version(output);
    let copy = b.block(BlockKind::Body, &[i, at_offset, version]);
    let append = b.block(BlockKind::Body, &[]);
    let done = b.block(BlockKind::Body, &[]);
    let reserved = b.reaching(output)?;
    b.jump(copy, &[zero, zero, reserved]);
    b.at(copy);
    b.set_reaching(output, version);
    let more = b.compare(BinaryOp::Lt, i, count)?;
    b.branch(more, append, done);
    b.at(append);
    let at = b.place(
        AddressBase::Value(source),
        &[ProjectionStep::FlatAt {
            index: i,
            offset: at_offset,
        }],
        element,
        Some("MEM-14"),
    )?;
    let end = b.value(word, Holding::Plain);
    b.push(Inst::FlatNext {
        result: end,
        address: at,
    });
    let failure = b.failure(None);
    let value = b.copy(at, failure)?;
    b.seq_append(destination, value)?;
    let next = advance(&mut b, i, one);
    let updated = b.reaching(output)?;
    b.jump(copy, &[next, end, updated]);
    b.at(done);
    b.set_reaching(output, version);
    b.jump(complete, &[version]);
    b.at(complete);
    b.set_reaching(output, output_version);
    // Close the loan on every failing path before destroying the partial
    // destination. This includes hypothetical failures of infallible scalar
    // instructions: C checks their declared failure edges too.
    let last_failure = b.ed.unit.tables.failures.len();
    for i in first_failure..last_failure {
        let old = b.ed.unit.tables.failures[i].cleanup;
        let cleanup = b.block(BlockKind::FailureCleanup, &[]);
        b.ed.code[cleanup.index()].push(Inst::EndLoan {
            loan,
            if_active: true,
        });
        b.ed.unit.terminators[cleanup.index()] = Terminator::Jump(old);
        b.ed.unit.tables.failures[i].cleanup = b.edge(cleanup, &[]);
    }
    b.end_loan(loan);
    // Prepare the replacement before moving the new owner out of its slot.
    let old = b.slot(list.layout, SlotKind::Displaced);
    b.push(Inst::StorageLive { slot: old });
    let old_at = b.place(AddressBase::Slot(old), &[], list.layout, None)?;
    let result = b.take(initial)?;
    b.emptied(output);
    b.push(Inst::StorageDead {
        slot: output,
        if_live: false,
    });
    let version = b.version(list.slot);
    let displaced_version = b.version(old);
    let failure = b.failure(None);
    b.push(Inst::Replace {
        address: list.address,
        value: result,
        displaced: old,
        version,
        displaced_version,
        if_live: false,
        failure,
    });
    b.set_reaching(list.slot, version);
    b.set_reaching(old, displaced_version);
    let anchor = CleanupAnchorId(b.ed.unit.tables.cleanup_anchors.len() as u32);
    b.ed.unit.tables.cleanup_anchors.push(CleanupAnchor {
        body: 0,
        cleanup: 0,
        action: 0,
        site: SiteRange {
            source: 0,
            start: 0,
            end: 0,
        },
    });
    let failure = b.failure(None);
    b.push(Inst::Drop {
        address: old_at,
        if_live: false,
        anchor,
        failure,
    });
    b.push(Inst::StorageDead {
        slot: old,
        if_live: false,
    });
    ret_unit(&mut b);
    Ok(b.finish())
}

// FlatAt replay accepts the checked source-language increment form. Keep the
// counter in that form rather than changing or weakening the replay proof.
fn advance(b: &mut Body<'_, '_>, index: ValueId, one: ValueId) -> ValueId {
    let result = b.value(b.layout_of(index), Holding::Plain);
    let failure = b.failure(None);
    b.push(Inst::Binary {
        result,
        op: BinaryOp::Add,
        left: index,
        right: one,
        failure,
    });
    result
}
