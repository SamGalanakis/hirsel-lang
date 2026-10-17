//! `list.len`, `array.len` and `string.byte_len`: an inline `SeqLen` of the
//! borrowed argument, the count of a List or Array block or the length of a
//! String slice. A `list.len` of a list
//! inside a flat value never reaches here: the importer reads its encoding's
//! count first (`FlatLength`, LIR schema revision 4).
use super::{CallShape, InlineLowering, R};
use crate::import::ImportCx;
use hirsel_lir::ir::{Holding, Inst};
use hirsel_types::UseKind;

pub static BYTES_LEN: InlineLowering = InlineLowering {
    path: || hirsel_syntax::DefPath::core("bytes.len"),
    lower: length,
};

pub static LIST_LEN: InlineLowering = InlineLowering {
    path: || hirsel_syntax::DefPath::core("list.len"),
    lower: length,
};

pub static ARRAY_LEN: InlineLowering = InlineLowering {
    path: || hirsel_syntax::DefPath::core("array.len"),
    lower: length,
};

pub static STRING_BYTE_LEN: InlineLowering = InlineLowering {
    path: || hirsel_syntax::DefPath::core("string.byte_len"),
    lower: length,
};

fn length(cx: &mut ImportCx<'_>, call: &CallShape<'_>) -> R<bool> {
    let [argument] = call.arguments else {
        return Ok(false);
    };
    if argument.value.usage != UseKind::Borrow {
        return Ok(false);
    }
    let value = cx.operand(&argument.value)?;
    let layout = cx.place_layout(call.destination)?;
    let result = cx.value(layout, Holding::Plain);
    cx.push(Inst::SeqLen { result, value });
    cx.consumed(value);
    cx.finish(call, result)?;
    Ok(true)
}
