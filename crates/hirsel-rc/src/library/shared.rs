//! `Shared[T]` roots (MEM-11). A root is a heap block whose `Frozen` payload
//! holds the value and whose header count is the root count: `Dup` and
//! `Release` of the reference add and give up roots, the last release thaws
//! the value and then releases it. Freezing makes every counted block the
//! value reaches immortal; thawing the last root restores their unique counts.
use super::LibraryBody;
use super::builder::{Body, Cx};
use super::slots::{construct, nominal};
use crate::PassError;
use hirsel_lir::ir::*;
use hirsel_lir::layout_types::LayoutKind;
use hirsel_lir::stage::Value;
type R<T> = Result<T, PassError>;
pub static FREEZE: LibraryBody = LibraryBody {
    path: || hirsel_syntax::DefPath::core("shared.freeze"),
    build: freeze,
};
pub static THAW: LibraryBody = LibraryBody {
    path: || hirsel_syntax::DefPath::core("shared.thaw"),
    build: thaw,
};
fn freeze(cx: &mut Cx<'_>) -> R<LirUnit<Value>> {
    let mut b = Body::new(cx)?;
    let value = b.take(b.parameters[0].address)?;
    let root = b.fresh(b.cx.fn_abi().result.unwrap().layout);
    b.push(Inst::Freeze {
        result: root,
        value,
    });
    b.ret(Some(root));
    Ok(b.finish())
}
/// `Ok` with the value when the root is the last, else `Error` with the root.
/// The root count answers alone: no credit but the roots' own keeps the block.
fn thaw(cx: &mut Cx<'_>) -> R<LirUnit<Value>> {
    let mut b = Body::new(cx)?;
    let result = b.cx.fn_abi().result.unwrap().layout;
    let root = b.take(b.parameters[0].address)?;
    let value_layout = match b.record(b.layout_of(root)).kind {
        LayoutKind::Shared { payload } => match b.record(payload).kind {
            LayoutKind::Frozen { value } => value,
            _ => {
                return Err(PassError::Malformed(
                    "a Shared payload that is not Frozen".into(),
                ));
            }
        },
        _ => {
            return Err(PassError::Malformed(
                "shared.thaw of a non-Shared root".into(),
            ));
        }
    };
    let bool_layout = b.bool_layout()?;
    let last = b.value(bool_layout, Holding::Plain);
    b.push(Inst::IsUnique {
        result: last,
        value: root,
    });
    let thawed = b.block(BlockKind::Body, &[]);
    let kept = b.block(BlockKind::Body, &[]);
    b.branch(last, thawed, kept);
    let definition = nominal(&mut b, "Result");
    b.at(thawed);
    let value = b.fresh(value_layout);
    b.push(Inst::Thaw {
        result: value,
        value: root,
    });
    let ok = construct(
        &mut b,
        result,
        ConstructorKind::Variant {
            definition,
            variant: 0,
        },
        &[value],
    )?;
    b.ret(Some(ok));
    b.at(kept);
    let error = construct(
        &mut b,
        result,
        ConstructorKind::Variant {
            definition,
            variant: 1,
        },
        &[root],
    )?;
    b.ret(Some(error));
    Ok(b.finish())
}
