//! SEM-3 string views retain the String slice and one holder credit.
use super::{CallShape, InlineLowering, R};
use crate::import::ImportCx;
use hirsel_lir::ir::Inst;
use hirsel_types::UseKind;

pub static AS_BYTES: InlineLowering = InlineLowering {
    path: || hirsel_syntax::DefPath::core("string.as_bytes"),
    lower: view,
};
pub static SCALARS: InlineLowering = InlineLowering {
    path: || hirsel_syntax::DefPath::core("string.scalars"),
    lower: view,
};
pub static POSITIONS: InlineLowering = InlineLowering {
    path: || hirsel_syntax::DefPath::core("string.positions"),
    lower: view,
};

fn view(cx: &mut ImportCx<'_>, call: &CallShape<'_>) -> R<bool> {
    let [argument] = call.arguments else {
        return Ok(false);
    };
    if argument.value.usage != UseKind::Borrow {
        return Ok(false);
    }
    let (address, reaching) = cx.place_address(argument.value.place)?;
    let layout = cx.place_layout(call.destination)?;
    let result = cx.fresh(layout);
    let failure = cx.failure(None)?;
    cx.push(Inst::Copy {
        result,
        address,
        reaching,
        failure,
    });
    cx.pending(result);
    cx.finish(call, result)?;
    Ok(true)
}
