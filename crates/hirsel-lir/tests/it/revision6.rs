//! LIR schema revision 6: the scalar core-library intrinsics are instructions
//! (`Convert`, `IntArith`, `MulAdd` and the named `BinaryOp`s), and a runtime
//! primitive's symbol template resolves per call (`abi::resolve_symbol`).
//! Every check has its rejected mutation and accepted twin built by one
//! function.
use crate::support;

use hirsel_core_contract::{Disposition, Passing};
use hirsel_lir::abi::resolve_symbol;
use hirsel_lir::ir::*;
use hirsel_lir::layout_types::{LayoutId, ScalarKind};
use hirsel_lir::stage::Ready;
use support::*;

fn borrow(layout: LayoutId) -> (Passing, LayoutId) {
    (Passing::Borrow, layout)
}

/// A flag value of `kind` (a Bool or the I32 status).
fn flag(t: &mut Lir<Ready>, kind: ScalarKind) -> ValueId {
    let layout = match kind {
        ScalarKind::Bool => l::BOOL,
        ScalarKind::I32 => l::I32,
        _ => l::U8,
    };
    t.value_as(ValueRepr::Scalar(kind), layout, Holding::Plain)
}

/// `convert(x: from) -> to` in `mode`, with a flag of `flag_kind`.
fn convert(
    from: LayoutId,
    to: LayoutId,
    mode: ConvertMode,
    flag_kind: Option<ScalarKind>,
) -> Vec<K> {
    let mut t = Lir::<Ready>::new(&[borrow(from)], Some(to));
    let x = t.plain(from);
    t.param(x);
    let result = t.plain(to);
    let flag = flag_kind.map(|k| flag(&mut t, k));
    t.push(
        ENTRY,
        Inst::Convert {
            result,
            flag,
            operand: x,
            mode,
        },
    );
    t.ret(ENTRY, Some(result));
    t.check()
}

#[test]
fn a_conversion_holds_only_between_the_kinds_its_mode_names() {
    let bad = |k: &K| *k == K::Type("conversion");
    // SEM-1 widen: the source's complete range fits; i64 does not fit i32.
    pair(
        |m| {
            let (from, to) = if m {
                (l::I64, l::I32)
            } else {
                (l::I32, l::I64)
            };
            convert(from, to, ConvertMode::Widen, None)
        },
        bad,
    );
    // Neither does a signed source fit an unsigned destination of any width.
    pair(
        |m| {
            convert(
                l::I32,
                if m { l::U64 } else { l::I64 },
                ConvertMode::Widen,
                None,
            )
        },
        bad,
    );
    // §22 truncate: a no-wider destination.
    pair(
        |m| {
            let (from, to) = if m {
                (l::I32, l::I64)
            } else {
                (l::I64, l::I32)
            };
            convert(from, to, ConvertMode::Truncate, None)
        },
        bad,
    );
    // try_narrow reports representability in a Bool flag, between any widths.
    pair(
        |m| {
            convert(
                l::I64,
                l::I32,
                ConvertMode::TryNarrow,
                (!m).then_some(ScalarKind::Bool),
            )
        },
        bad,
    );
    pair(
        |m| {
            convert(
                l::I32,
                l::U64,
                ConvertMode::TryNarrow,
                Some(if m { ScalarKind::U8 } else { ScalarKind::Bool }),
            )
        },
        bad,
    );
    // Widen and truncate have no flag.
    pair(
        |m| {
            convert(
                l::I32,
                l::I64,
                ConvertMode::Widen,
                m.then_some(ScalarKind::Bool),
            )
        },
        bad,
    );
    // float.to_i32: a float into i32 with an I32 status.
    pair(
        |m| {
            convert(
                if m { l::I64 } else { l::F64 },
                l::I32,
                ConvertMode::FloatToI32,
                Some(ScalarKind::I32),
            )
        },
        bad,
    );
    pair(
        |m| {
            convert(
                l::F64,
                if m { l::I64 } else { l::I32 },
                ConvertMode::FloatToI32,
                Some(ScalarKind::I32),
            )
        },
        bad,
    );
    pair(
        |m| {
            convert(
                l::F64,
                l::I32,
                ConvertMode::FloatToI32,
                Some(if m { ScalarKind::Bool } else { ScalarKind::I32 }),
            )
        },
        bad,
    );
}

/// `a op b` of `layout` in `mode`, with a flag when `with_flag`.
fn int_arith(layout: LayoutId, op: BinaryOp, mode: ArithMode, with_flag: bool) -> Vec<K> {
    let mut t = Lir::<Ready>::new(&[borrow(layout), borrow(layout)], Some(layout));
    let (a, b) = (t.plain(layout), t.plain(layout));
    t.param(a);
    t.param(b);
    let result = t.plain(layout);
    let flag = with_flag.then(|| flag(&mut t, ScalarKind::Bool));
    t.push(
        ENTRY,
        Inst::IntArith {
            result,
            flag,
            op,
            mode,
            left: a,
            right: b,
        },
    );
    t.ret(ENTRY, Some(result));
    t.check()
}

#[test]
fn integer_arithmetic_adds_integers_and_flags_exactly_a_checked_overflow() {
    let bad = |k: &K| *k == K::Type("integer arithmetic");
    // `Add` is the only operation the library names (§22).
    pair(
        |m| {
            int_arith(
                l::I64,
                if m { BinaryOp::Sub } else { BinaryOp::Add },
                ArithMode::Wrapping,
                false,
            )
        },
        bad,
    );
    pair(
        |m| {
            int_arith(
                if m { l::F64 } else { l::I64 },
                BinaryOp::Add,
                ArithMode::Saturating,
                false,
            )
        },
        bad,
    );
    // Checked mode reports overflow in its flag; the others have none.
    pair(
        |m| int_arith(l::I64, BinaryOp::Add, ArithMode::Checked, !m),
        bad,
    );
    pair(
        |m| int_arith(l::I64, BinaryOp::Add, ArithMode::Wrapping, m),
        bad,
    );
}

/// `mul_add(a, b, c)` of `layout`.
fn mul_add(layout: LayoutId) -> Vec<K> {
    let mut t = Lir::<Ready>::new(&[borrow(layout); 3], Some(layout));
    let operands: Vec<_> = (0..3).map(|_| t.plain(layout)).collect();
    for &v in &operands {
        t.param(v);
    }
    let result = t.plain(layout);
    t.push(
        ENTRY,
        Inst::MulAdd {
            result,
            left: operands[0],
            right: operands[1],
            addend: operands[2],
        },
    );
    t.ret(ENTRY, Some(result));
    t.check()
}

#[test]
fn a_fused_multiply_add_is_of_one_float_kind() {
    pair(
        |m| mul_add(if m { l::I64 } else { l::F64 }),
        |k| *k == K::Type("fused multiply-add"),
    );
}

/// `a op b` of `layout` into `result`.
fn binary(layout: LayoutId, op: BinaryOp, result: LayoutId) -> Vec<K> {
    let mut t = Lir::<Ready>::new(&[borrow(layout), borrow(layout)], Some(result));
    let (a, b) = (t.plain(layout), t.plain(layout));
    t.param(a);
    t.param(b);
    let r = t.plain(result);
    let failure = t.fails();
    t.push(
        ENTRY,
        Inst::Binary {
            result: r,
            op,
            left: a,
            right: b,
            failure,
        },
    );
    t.ret(ENTRY, Some(r));
    t.check()
}

#[test]
fn the_named_binary_operations_hold_only_in_their_domain() {
    let bad = |k: &K| *k == K::Type("binary operands");
    for op in [BinaryOp::DivFloor, BinaryOp::Mod, BinaryOp::DivExact] {
        pair(
            |m| {
                let layout = if m { l::F64 } else { l::I64 };
                binary(layout, op, layout)
            },
            bad,
        );
    }
    for op in [
        BinaryOp::Min,
        BinaryOp::Max,
        BinaryOp::MinNumber,
        BinaryOp::MaxNumber,
    ] {
        pair(
            |m| {
                let layout = if m { l::I64 } else { l::F64 };
                binary(layout, op, layout)
            },
            bad,
        );
    }
    // totalOrder yields an i32 -1, 0 or 1.
    pair(
        |m| {
            binary(
                l::F64,
                BinaryOp::TotalOrder,
                if m { l::F64 } else { l::I32 },
            )
        },
        bad,
    );
    pair(
        |m| {
            binary(
                if m { l::I64 } else { l::F64 },
                BinaryOp::TotalOrder,
                l::I32,
            )
        },
        bad,
    );
}

/// `op(x: i64) -> result` at Ready: a call of the fixture operation with
/// `native` disposition, or (`instruction`) its `Convert` spelling.
fn ready_operation(native: Disposition, result: LayoutId, instruction: bool) -> Vec<K> {
    let mut t = Lir::<Ready>::new(&[borrow(l::I64)], Some(result));
    t.manifest = manifest(native);
    let x = t.plain(l::I64);
    t.param(x);
    if instruction {
        let r = t.plain(result);
        t.push(
            ENTRY,
            Inst::Convert {
                result: r,
                flag: None,
                operand: x,
                mode: ConvertMode::Truncate,
            },
        );
        t.ret(ENTRY, Some(r));
        return t.check();
    }
    let n = t.block(BlockKind::Body, &[]);
    let fail = t.propagate();
    let abi = t.abi(&[borrow(l::I64)], Some(result));
    let result_type = if result == l::I32 {
        hirsel_types::Primitive::I32
    } else if result == l::F64 {
        hirsel_types::Primitive::F64
    } else {
        hirsel_types::Primitive::I64
    };
    t.declare_operation(
        super::operation_abi::scalar_signature(
            "fixture",
            &[(
                hirsel_syntax::PassingMode::Borrow,
                hirsel_types::Primitive::I64,
            )],
            Some(result_type),
        ),
        Default::default(),
        abi,
    );
    let r = t
        .call_to(
            ENTRY,
            Callee::Operation(OperationRef(0)),
            abi,
            &[borrow(l::I64)],
            Some(result),
            &[ArgumentValue::Value(x)],
            (n, &[]),
            fail,
        )
        .unwrap();
    t.ret(n, Some(r));
    t.check()
}

#[test]
fn ready_spells_a_scalar_intrinsic_as_its_instruction() {
    pair(
        |m| ready_operation(Disposition::Instruction("int.truncate".into()), l::I32, !m),
        |k| *k == K::UnsupportedOperation(OperationRef(0)),
    );
}

#[test]
fn ready_calls_a_runtime_template_only_when_it_resolves() {
    // `{I}` reads the integer argument: i64 resolves, and an unknown
    // placeholder never does.
    let template = |t: &str| Disposition::RuntimePrimitive(t.into());
    pair(
        |m| {
            ready_operation(
                template(if m {
                    "hirsel_rt_v2_{K}_op"
                } else {
                    "hirsel_rt_v2_{I}_op"
                }),
                l::I32,
                false,
            )
        },
        |k| *k == K::UnsupportedOperation(OperationRef(0)),
    );
    // `{F}` needs a float argument or result.
    pair(
        |m| {
            ready_operation(
                template("hirsel_rt_v2_{F}_sum"),
                if m { l::I32 } else { l::F64 },
                false,
            )
        },
        |k| *k == K::UnsupportedOperation(OperationRef(0)),
    );
}

#[test]
fn symbol_templates_resolve_to_sized_scalar_names() {
    use ScalarKind as S;
    assert_eq!(
        resolve_symbol("hirsel_rt_v2_{F}_sum", &[], Some(S::F64)).as_deref(),
        Some("hirsel_rt_v2_f64_sum")
    );
    assert_eq!(
        resolve_symbol("hirsel_rt_v2_{F}_sum_exact", &[], Some(S::F32)).as_deref(),
        Some("hirsel_rt_v2_f32_sum_exact")
    );
    assert_eq!(
        resolve_symbol("hirsel_rt_v2_{I}_to_{J}_x", &[S::I128], Some(S::U8)).as_deref(),
        Some("hirsel_rt_v2_i128_to_u8_x")
    );
    // A concrete name is its own resolution.
    assert_eq!(
        resolve_symbol("hirsel_rt_v2_i32_order", &[S::I32, S::I32], Some(S::I32)).as_deref(),
        Some("hirsel_rt_v2_i32_order")
    );
    // No kind of the placeholder's domain, unknown placeholders and unbalanced
    // braces resolve to nothing.
    for (template, arguments, result) in [
        ("hirsel_rt_v2_{F}_sum", &[][..], Some(S::I64)),
        ("hirsel_rt_v2_{F}_sum", &[][..], None),
        ("hirsel_rt_v2_{I}_x", &[S::F32][..], None),
        ("hirsel_rt_v2_{I}_x", &[S::Bool][..], None),
        ("hirsel_rt_v2_{J}_x", &[S::I8][..], Some(S::Pointer)),
        ("hirsel_rt_v2_{T}_x", &[S::I8][..], Some(S::I8)),
        ("hirsel_rt_v2_{I_x", &[S::I8][..], Some(S::I8)),
        ("hirsel_rt_v2_I}_x", &[S::I8][..], Some(S::I8)),
    ] {
        assert_eq!(
            resolve_symbol(template, arguments, result),
            None,
            "{template}"
        );
    }
}

#[test]
fn int_to_float_requires_an_integer_input_float_result_and_no_flag() {
    let bad = |k: &K| *k == K::Type("conversion");
    for (from, to, flag) in [
        (l::F64, l::F64, None),
        (l::BOOL, l::F64, None),
        (l::I64, l::I32, None),
        (l::I64, l::F64, Some(ScalarKind::Bool)),
    ] {
        pair(
            |mutant| {
                if mutant {
                    convert(from, to, ConvertMode::IntToFloat, flag)
                } else {
                    convert(l::I64, l::F64, ConvertMode::IntToFloat, None)
                }
            },
            bad,
        );
    }
    for from in [l::I32, l::I64, l::U64] {
        assert!(convert(from, l::F64, ConvertMode::IntToFloat, None).is_empty());
    }
}

#[test]
fn reinterpret_pairs_each_float_with_its_unsigned_word_and_has_no_flag() {
    let bad = |k: &K| *k == K::Type("conversion");
    for (from, to, flag) in [
        (l::F64, l::I64, None),
        (l::I64, l::F64, None),
        (l::F64, l::U32, None),
        (l::U32, l::F64, None),
        (l::F32, l::U64, None),
        (l::F64, l::F64, None),
        (l::U64, l::U64, None),
        (l::F64, l::U64, Some(ScalarKind::Bool)),
    ] {
        pair(
            |mutant| {
                if mutant {
                    convert(from, to, ConvertMode::Reinterpret, flag)
                } else {
                    convert(l::F64, l::U64, ConvertMode::Reinterpret, None)
                }
            },
            bad,
        );
    }
    for (from, to) in [
        (l::F64, l::U64),
        (l::U64, l::F64),
        (l::F32, l::U32),
        (l::U32, l::F32),
    ] {
        let violations = convert(from, to, ConvertMode::Reinterpret, None);
        assert!(violations.is_empty(), "{from:?} to {to:?}: {violations:?}");
    }
}
