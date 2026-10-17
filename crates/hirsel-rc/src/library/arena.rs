//! `arena.with[R](body)`: open an arena, call `body` with its token, and
//! close the arena on both outcomes (MEM-4).
//! The opening is `ArenaOpen` at the unit's entry; placement lowers it to a
//! region entry and token, and closes that region on every return and
//! propagation, before the result leaves the frame.
use super::LibraryBody;
use super::builder::{Body, Cx};
use crate::PassError;
use hirsel_core_contract::Passing;
use hirsel_lir::abi_types::CallableShape;
use hirsel_lir::ir::*;
use hirsel_lir::stage::Value;
use hirsel_types::TypeKind;

pub static WITH: LibraryBody = LibraryBody {
    path: || hirsel_syntax::DefPath::core("arena.with"),
    build: with,
};

fn with(cx: &mut Cx<'_>) -> Result<LirUnit<Value>, PassError> {
    let callback = match cx.store.get(cx.callable).map(|t| &t.kind) {
        Some(TypeKind::Function(f)) if f.parameters.len() == 1 => f.parameters[0].ty,
        _ => {
            return Err(PassError::Malformed("arena.with takes one callback".into()));
        }
    };
    let abi = cx
        .layouts
        .fn_abi(&cx.store, callback, CallableShape::Closure)
        .map_err(|e| PassError::Layout(cx.path.clone(), e))?;
    let shape = cx.layouts.table().fn_abis[abi.0 as usize].clone();
    let [token_parameter] = shape.parameters.as_slice() else {
        return Err(PassError::Malformed(
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

/// `heap` is an inert allocator token, represented independently of any region.
pub static HEAP: super::InlineLowering = super::InlineLowering {
    path: || hirsel_syntax::DefPath::core("$intrinsic.heap"),
    lower: |cx, call| {
        let layout = cx.place_layout(call.destination)?;
        let token = cx.constant(layout, ConstantValue::Heap);
        cx.finish(call, token)?;
        Ok(true)
    },
};

pub static COPY_TO: LibraryBody = LibraryBody {
    path: || hirsel_syntax::DefPath::core("copy_to"),
    build: copy_to,
};

fn copy_to(cx: &mut Cx<'_>) -> Result<LirUnit<Value>, PassError> {
    let mut b = Body::new(cx)?;
    let source = b.parameters[1];
    let (value, loan) = b.borrow(source.address, false)?;
    let result = b.fresh(source.layout);
    let layout = b.cx.payload(source.layout).unwrap_or(source.layout);
    let construct = ConstructId(b.ed.unit.tables.constructs.len() as u32);
    b.ed.unit.tables.constructs.push(ConstructSite {
        arena_no_drop: false,
        constructor: ConstructorKind::Tuple,
        layout,
        fields: PoolRange::EMPTY,
        destination: DestinationClass::Arena(b.parameters[0].slot),
        placement: None,
        anchor: b.anchor,
    });
    let next = b.failure(None);
    let cleanup = b.ed.unit.tables.failures[next.index()].cleanup;
    let ending = b.block(BlockKind::FailureCleanup, &[]);
    b.ed.code[ending.index()].push(Inst::EndLoan {
        loan,
        if_active: false,
    });
    b.ed.unit.terminators[ending.index()] = Terminator::Jump(cleanup);
    let failure = b.failure_to(ending, &[], None);
    b.push(Inst::UniqueCopy {
        result,
        value,
        construct,
        deep: true,
        failure,
    });
    b.end_loan(loan);
    b.ret(Some(result));
    Ok(b.finish())
}
