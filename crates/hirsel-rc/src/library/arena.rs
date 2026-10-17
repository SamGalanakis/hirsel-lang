//! `arena.with[R](body)`: open an arena, call `body` with its token, and
//! close the arena on both outcomes (MEM-4); `arena.bounded[R](cap, body)`
//! opens it bounded and recovers its own `OutOfSpace` (MEM-6).
//! The opening is `ArenaOpen` at the unit's entry; placement lowers it to a
//! region entry and token, and closes that region on every return and
//! propagation, before the result leaves the frame.
use super::LibraryBody;
use super::builder::{Body, Cx};
use crate::PassError;
use hirsel_core_contract::Passing;
use hirsel_lir::abi_types::CallableShape;
use hirsel_lir::ir::*;
use hirsel_lir::layout_types::LayoutId;
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
    b.ed.code[entry].insert(
        0,
        Inst::ArenaOpen {
            result: token,
            budget: None,
        },
    );
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

pub static BOUNDED: LibraryBody = LibraryBody {
    path: || hirsel_syntax::DefPath::core("arena.bounded"),
    build: bounded,
};

const RESULT_OK: u32 = 0;
const RESULT_ERROR: u32 = 1;

/// `arena.bounded[R](cap, body)` (MEM-6): `arena.with`'s shape over an arena
/// bounded by `cap`. The callback's result is `Ok`; its failure ends the loan,
/// then recovers exactly that arena's own `OutOfSpace` as `Error(OutOfSpace())`
/// and propagates every other failure.
fn bounded(cx: &mut Cx<'_>) -> Result<LirUnit<Value>, PassError> {
    let callback = match cx.store.get(cx.callable).map(|t| &t.kind) {
        Some(TypeKind::Function(f)) if f.parameters.len() == 2 => f.parameters[1].ty,
        _ => {
            return Err(PassError::Malformed(
                "arena.bounded takes a capacity and one callback".into(),
            ));
        }
    };
    let abi = cx
        .layouts
        .fn_abi(&cx.store, callback, CallableShape::Closure)
        .map_err(|e| PassError::Layout(cx.path.clone(), e))?;
    let shape = cx.layouts.table().fn_abis[abi.0 as usize].clone();
    let [token_parameter] = shape.parameters.as_slice() else {
        return Err(PassError::Malformed(
            "an arena.bounded callback takes one arena".into(),
        ));
    };
    let Some(returned) = cx.fn_abi().result.map(|r| r.layout) else {
        return Err(PassError::Malformed(
            "arena.bounded returns a Result".into(),
        ));
    };
    let mut b = Body::new(cx)?;
    let token = b.fresh(token_parameter.layout);
    // The opening follows the parameters' preparation, so it reads its budget.
    let start = b.current();
    b.at(b.ed.unit.entry);
    let cap = b.read_bits(b.parameters[0].address)?;
    b.push(Inst::ArenaOpen {
        result: token,
        budget: Some(cap),
    });
    b.at(start);
    let (callee, loan) = b.borrow(b.parameters[1].address, false)?;
    let call = b.current();
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
    let ok = match result {
        Some(value) => vec![value],
        None => match payload(&b, returned, RESULT_OK) {
            Some(unit) => vec![b.constant(unit, ConstantValue::Unit)],
            None => Vec::new(),
        },
    };
    let value = variant(&mut b, returned, "Result", RESULT_OK, &ok);
    b.ret(Some(value));
    // The callback's failure path ends the loan, then recovers.
    let Terminator::Call { failure, .. } = b.ed.unit.terminators[call.index()] else {
        return Err(PassError::Malformed(
            "arena.bounded calls its callback".into(),
        ));
    };
    let ending = b.ed.unit.tables.failures[failure.index()].cleanup;
    let ending = b.ed.unit.tables.edges[ending.index()].target;
    let Terminator::Jump(failed) = b.ed.unit.terminators[ending.index()] else {
        return Err(PassError::Malformed(
            "arena.bounded's callback failure continues into cleanup".into(),
        ));
    };
    let recovered = b.block(BlockKind::Body, &[]);
    let caught = b.edge(recovered, &[]);
    b.ed.unit.terminators[ending.index()] = Terminator::Recover {
        arena: token,
        caught,
        failed,
    };
    b.at(recovered);
    let error = payload(&b, returned, RESULT_ERROR).ok_or_else(|| {
        PassError::Malformed("arena.bounded's Result has no error payload".into())
    })?;
    let out_of_space = variant(&mut b, error, "OutOfSpace", 0, &[]);
    let value = variant(&mut b, returned, "Result", RESULT_ERROR, &[out_of_space]);
    b.ret(Some(value));
    Ok(b.finish())
}

/// The layout of field 0 of `layout`'s variant `ordinal`.
fn payload(b: &Body<'_, '_>, layout: LayoutId, ordinal: u32) -> Option<LayoutId> {
    let variant = b
        .record(layout)
        .tag
        .as_ref()?
        .variants
        .iter()
        .find(|v| v.ordinal == ordinal)?;
    variant
        .fields
        .iter()
        .find(|f| f.ordinal == 0)
        .map(|f| f.layout)
}

/// Construct variant `variant` of the core type `name` at `layout`.
fn variant(
    b: &mut Body<'_, '_>,
    layout: LayoutId,
    name: &str,
    variant: u32,
    values: &[ValueId],
) -> ValueId {
    let definition = b.symbol(Symbol {
        kind: SymbolKind::Type,
        definition: hirsel_syntax::DefPath::core(name),
        instance: None,
    });
    let fields = PoolRange::push(
        &mut b.ed.unit.pools.fields,
        values.iter().enumerate().map(|(i, &value)| FieldValue {
            field: i as u32,
            value,
        }),
    );
    let construct = ConstructId(b.ed.unit.tables.constructs.len() as u32);
    b.ed.unit.tables.constructs.push(ConstructSite {
        arena_no_drop: false,
        constructor: ConstructorKind::Variant {
            definition,
            variant,
        },
        layout,
        fields,
        destination: DestinationClass::Inferred,
        placement: None,
        anchor: b.anchor,
    });
    let result = b.fresh(layout);
    let failure = b.failure(None);
    b.push(Inst::Construct {
        result,
        construct,
        failure,
    });
    result
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
