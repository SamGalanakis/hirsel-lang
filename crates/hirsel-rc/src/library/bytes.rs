//! Freeze a typed byte list by transferring its owner into a whole Bytes slice.
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
        .map_err(|e| crate::import::ImportError::Malformed(format!("{e:?}")))?;
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
