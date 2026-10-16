//! Freeze a typed byte list by transferring its owner into a whole Bytes slice.
use super::{CallShape, InlineLowering, R, core_path};
use crate::import::ImportCx;
use hirsel_lir::ir::{ConstantValue, ConstructorKind, Holding, Inst};
use hirsel_types::UseKind;

pub static FROM_LIST: InlineLowering = InlineLowering {
    path: || core_path("bytes.from_list"),
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
    let zero = cx.constant(word, ConstantValue::Integer(0));
    let result = cx.construct(
        layout,
        layout,
        ConstructorKind::Tuple,
        &[owner, zero, length],
    )?;
    cx.finish(call, result)?;
    Ok(true)
}
