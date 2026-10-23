//! Total scalar/byte-vector core calls expand in the caller, with no runtime ABI.
use super::{CallShape, InlineLowering, R};
use crate::{PassError, import::ImportCx};
use hirsel_lir::{ir::*, layout_types::LayoutId};
use hirsel_types::{GenericArgument, Primitive, Size, UseKind};

macro_rules! lowering {
    ($($constant:ident => $path:literal),* $(,)?) => { $(
        pub static $constant: InlineLowering = InlineLowering { path: || hirsel_syntax::DefPath::core($path), lower };
    )* };
}
lowering! {
    U16_LE => "bytes.try_read_u16_le", U16_BE => "bytes.try_read_u16_be",
    U32_LE => "bytes.try_read_u32_le", U32_BE => "bytes.try_read_u32_be",
    U64_LE => "bytes.try_read_u64_le", U64_BE => "bytes.try_read_u64_be",
    LOAD => "simd.try_load_bytes", EQ => "simd.eq_bytes", OR => "simd.bit_or", MASK => "simd.bitmask",
}

fn some_layout(cx: &ImportCx<'_>, option: LayoutId) -> R<LayoutId> {
    cx.record(option)
        .tag
        .as_ref()
        .and_then(|t| t.variants.iter().find(|v| v.ordinal == 1))
        .and_then(|v| v.fields.first())
        .map(|f| f.layout)
        .ok_or_else(|| PassError::Malformed("total byte read result without Some field".into()))
}
fn lower(cx: &mut ImportCx<'_>, call: &CallShape<'_>) -> R<bool> {
    let name = call.path.0.name.as_str();
    let scalar = [
        ("try_read_u16_le", BytesWidth::U16, ByteOrder::Little),
        ("try_read_u16_be", BytesWidth::U16, ByteOrder::Big),
        ("try_read_u32_le", BytesWidth::U32, ByteOrder::Little),
        ("try_read_u32_be", BytesWidth::U32, ByteOrder::Big),
        ("try_read_u64_le", BytesWidth::U64, ByteOrder::Little),
        ("try_read_u64_be", BytesWidth::U64, ByteOrder::Big),
    ]
    .into_iter()
    .find(|(n, _, _)| *n == name);
    let lanes = if scalar.is_none() {
        match call.generics {
            [GenericArgument::Size(Size::Constant(n @ 1..=64))] => *n as u8,
            _ => {
                return Err(PassError::Malformed(
                    "byte SIMD width must be a constant in 1..=64".into(),
                ));
            }
        }
    } else {
        0
    };
    let destination = cx.place_layout(call.destination)?;
    if scalar.is_some() || name == "try_load_bytes" {
        let [data, at] = call.arguments else {
            return Ok(false);
        };
        if data.value.usage != UseKind::Borrow {
            return Ok(false);
        }
        let bytes = cx.operand(&data.value)?;
        let offset = cx.operand(&at.value)?;
        let result = cx.value(some_layout(cx, destination)?, Holding::Plain);
        let boolean = cx.primitive(Primitive::Bool)?;
        let valid = cx.value(boolean, Holding::Plain);
        if let Some((_, width, endian)) = scalar {
            cx.push(Inst::BytesRead {
                result,
                valid,
                bytes,
                offset,
                width,
                endian,
            });
        } else {
            cx.push(Inst::BytesVectorRead {
                result,
                valid,
                bytes,
                offset,
                lanes,
            });
        }
        cx.consumed(bytes);
        cx.consumed(offset);
        let option = cx.checked_option(destination, valid, result)?;
        cx.finish(call, option)?;
    } else {
        let values = call
            .arguments
            .iter()
            .map(|a| cx.operand(&a.value))
            .collect::<R<Vec<_>>>()?;
        let op = match (name, values.as_slice()) {
            ("eq_bytes", &[left, right]) => ByteVectorOp::Eq { left, right },
            ("bit_or", &[left, right]) => ByteVectorOp::Or { left, right },
            ("bitmask", &[value]) => ByteVectorOp::Bitmask { value },
            _ => return Err(PassError::Malformed("byte SIMD operands".into())),
        };
        let result = cx.value(destination, Holding::Plain);
        cx.push(Inst::ByteVector { result, op, lanes });
        for value in values {
            cx.consumed(value);
        }
        cx.finish(call, result)?;
    }
    Ok(true)
}

/// Function values of these operations have the same instruction implementation.
pub(super) fn supports(path: &hirsel_syntax::DefPath) -> bool {
    [
        "bytes.try_read_u16_le",
        "bytes.try_read_u16_be",
        "bytes.try_read_u32_le",
        "bytes.try_read_u32_be",
        "bytes.try_read_u64_le",
        "bytes.try_read_u64_be",
        "simd.try_load_bytes",
        "simd.eq_bytes",
        "simd.bit_or",
        "simd.bitmask",
    ]
    .iter()
    .any(|name| path.is_core(name))
}
pub(super) fn forward(
    cx: &mut super::Cx<'_>,
) -> R<hirsel_lir::ir::LirUnit<hirsel_lir::stage::Value>> {
    use super::builder::Body;
    let name = cx.path.0.name.clone();
    let read = name.starts_with("try_read_") || name == "try_load_bytes";
    let lanes = if name.starts_with("try_read_") {
        0
    } else {
        let n = cx.size_actual(0)?;
        if !(1..=64).contains(&n) {
            return Err(PassError::Malformed("byte SIMD width".into()));
        }
        n as u8
    };
    let mut b = Body::new(cx)?;
    let destination =
        b.cx.fn_abi()
            .result
            .ok_or_else(|| PassError::Malformed("byte operation result".into()))?
            .layout;
    if read {
        let data = b.parameters[0];
        let at = b.parameters[1];
        let (bytes, loan) = b.borrow(data.address, false)?;
        let offset = b.read_bits(at.address)?;
        let root = b
            .record(destination)
            .tag
            .as_ref()
            .and_then(|t| t.variants.iter().find(|v| v.ordinal == 1))
            .and_then(|v| v.fields.first())
            .ok_or_else(|| PassError::Malformed("byte read Some layout".into()))?
            .layout;
        let result = b.value(root, Holding::Plain);
        let boolean = b.bool_layout()?;
        let valid = b.value(boolean, Holding::Plain);
        if lanes == 0 {
            let width = if name.contains("u16") {
                BytesWidth::U16
            } else if name.contains("u32") {
                BytesWidth::U32
            } else {
                BytesWidth::U64
            };
            let endian = if name.ends_with("_be") {
                ByteOrder::Big
            } else {
                ByteOrder::Little
            };
            b.push(Inst::BytesRead {
                result,
                valid,
                bytes,
                offset,
                width,
                endian,
            });
        } else {
            b.push(Inst::BytesVectorRead {
                result,
                valid,
                bytes,
                offset,
                lanes,
            });
        }
        b.end_loan(loan);
        let yes = b.block(BlockKind::Body, &[]);
        let no = b.block(BlockKind::Body, &[]);
        b.branch(valid, yes, no);
        b.at(yes);
        let some = super::map::option(&mut b, Some(result))?;
        b.ret(Some(some));
        b.at(no);
        let none = super::map::option(&mut b, None)?;
        b.ret(Some(none));
    } else {
        let mut values = Vec::new();
        for parameter in b.parameters.clone() {
            values.push(b.read_bits(parameter.address)?);
        }
        let op = match values.as_slice() {
            &[value] => ByteVectorOp::Bitmask { value },
            &[left, right] if name == "eq_bytes" => ByteVectorOp::Eq { left, right },
            &[left, right] => ByteVectorOp::Or { left, right },
            _ => return Err(PassError::Malformed("byte operation arguments".into())),
        };
        let result = b.value(destination, Holding::Plain);
        b.push(Inst::ByteVector { result, op, lanes });
        b.ret(Some(result));
    }
    Ok(b.finish())
}
