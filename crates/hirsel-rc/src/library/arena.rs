//! `arena.with[R](body)`: open an arena, call `body` with its token, and
//! close the arena on both outcomes (MEM-4).
//! The opening is `ArenaOpen` at the unit's entry; placement lowers it to a
//! region entry and token, and closes that region on every return and
//! propagation, before the result leaves the frame.
use super::builder::{Body, Cx};
use super::{LibraryBody, LibraryError, core_path};
use hirsel_core_contract::Passing;
use hirsel_lir::abi_types::CallableShape;
use hirsel_lir::ir::*;
use hirsel_lir::stage::Value;
use hirsel_types::TypeKind;

pub static WITH: LibraryBody = LibraryBody {
    path: || core_path("arena.with"),
    build: with,
};

fn with(cx: &mut Cx<'_>) -> Result<LirUnit<Value>, LibraryError> {
    let callback = match cx.store.get(cx.callable).map(|t| &t.kind) {
        Some(TypeKind::Function(f)) if f.parameters.len() == 1 => f.parameters[0].ty,
        _ => {
            return Err(LibraryError::Malformed(
                "arena.with takes one callback".into(),
            ));
        }
    };
    let abi = cx
        .layouts
        .fn_abi(&cx.store, callback, CallableShape::Closure)
        .map_err(|e| LibraryError::Layout(cx.path.clone(), e))?;
    let shape = cx.layouts.table().fn_abis[abi.0 as usize].clone();
    let [token_parameter] = shape.parameters.as_slice() else {
        return Err(LibraryError::Malformed(
            "an arena.with callback takes one arena".into(),
        ));
    };
    let mut b = Body::new(cx)?;
    let token = b.fresh(token_parameter.layout);
    let entry = b.ed.unit.entry.index();
    b.ed.code[entry].insert(0, Inst::ArenaOpen { result: token });
    let (callee, loan) = b.borrow(b.parameters[0].address, false)?;
    let result = super::iter::call(
        &mut b,
        callee,
        abi,
        vec![CallArgument {
            parameter: 0,
            passing: Passing::Borrow,
            value: ArgumentValue::Value(token),
        }],
        &[loan],
    )?;
    b.ret(result);
    Ok(b.finish())
}
