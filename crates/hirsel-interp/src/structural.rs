//! Iterative read-only traversal. Constants are provisional under spec issue #16.
use crate::{
    Value,
    scalar::{self, ScalarError},
};
use hirsel_syntax::BinaryOp;
use std::cmp::Ordering;
pub const NATIVE_DEPTH: usize = 64;
pub const SCRATCH_LEVELS: usize = 1024;
pub const MAX_DEPTH: usize = NATIVE_DEPTH + SCRATCH_LEVELS;
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StructuralError {
    Depth,
    InvalidValue,
}
fn children(v: &Value) -> Option<&[Option<Value>]> {
    match v {
        Value::Map(f)
        | Value::Set(f)
        | Value::Tuple(f)
        | Value::Box(f)
        | Value::Record { fields: f, .. }
        | Value::Variant { fields: f, .. }
        | Value::Distinct { fields: f, .. }
        | Value::Sequence { elements: f, .. } => Some(f),
        _ => None,
    }
}
fn identity(a: &Value, b: &Value) -> Result<Ordering, StructuralError> {
    match (a, b) {
        (Value::Map(_), Value::Map(_))
        | (Value::Set(_), Value::Set(_))
        | (Value::Tuple(_), Value::Tuple(_))
        | (Value::Box(_), Value::Box(_)) => Ok(Ordering::Equal),
        (Value::Record { definition: a, .. }, Value::Record { definition: b, .. })
        | (Value::Distinct { definition: a, .. }, Value::Distinct { definition: b, .. })
            if a == b =>
        {
            Ok(Ordering::Equal)
        }
        (Value::Variant { variant: a, .. }, Value::Variant { variant: b, .. }) => Ok(a.cmp(b)),
        (Value::Sequence { kind: a, .. }, Value::Sequence { kind: b, .. }) if a == b => {
            Ok(Ordering::Equal)
        }
        _ => Err(StructuralError::InvalidValue),
    }
}
/// None is IEEE unordered; language equality never uses Rust bit equality.
pub fn compare(a: &Value, b: &Value) -> Result<Option<Ordering>, StructuralError> {
    compare_at(a, b, 0, false)
}
fn compare_at(
    a: &Value,
    b: &Value,
    initial_depth: usize,
    equality_only: bool,
) -> Result<Option<Ordering>, StructuralError> {
    enum Work<'a> {
        Pair(&'a Value, &'a Value, usize),
        Length(usize, usize),
    }
    let mut work = vec![Work::Pair(a, b, initial_depth)];
    while let Some(w) = work.pop() {
        match w {
            Work::Length(a, b) => {
                if a != b {
                    return Ok(Some(a.cmp(&b)));
                }
            }
            Work::Pair(a, b, depth) => {
                if depth > MAX_DEPTH {
                    return Err(StructuralError::Depth);
                }
                if matches!(
                    a,
                    Value::Resource { .. }
                        | Value::ResourceVariant { .. }
                        | Value::Closure { .. }
                        | Value::FunctionReference(_)
                        | Value::Handle { .. }
                ) || matches!(
                    b,
                    Value::Resource { .. }
                        | Value::ResourceVariant { .. }
                        | Value::Closure { .. }
                        | Value::FunctionReference(_)
                        | Value::Handle { .. }
                ) {
                    return Err(StructuralError::InvalidValue);
                }
                if matches!((a, b), (Value::Set(_), Value::Set(_))) {
                    // Canonical encoding proves equality even for hand-built
                    // heterogeneous sets; its byte order is not structural order.
                    if equality_only {
                        let order = encoding(a, depth)?.cmp(&encoding(b, depth)?);
                        if order != Ordering::Equal {
                            return Ok(Some(order));
                        }
                        continue;
                    }
                    // SEM-5, spec #93: canonical structural element order,
                    // independent of insertion order and encoding byte artifacts.
                    let (Value::Set(ac), Value::Set(bc)) = (a, b) else {
                        unreachable!()
                    };
                    fn canonical(
                        fields: &[Option<Value>],
                        depth: usize,
                    ) -> Result<Vec<&Value>, StructuralError> {
                        let mut values: Vec<_> = fields
                            .iter()
                            .map(|v| v.as_ref().ok_or(StructuralError::InvalidValue))
                            .collect::<Result<_, _>>()?;
                        let mut failure = None;
                        values.sort_by(|a, b| match compare_at(a, b, depth + 1, false) {
                            Ok(Some(order)) => order,
                            Ok(None) => {
                                failure = Some(StructuralError::InvalidValue);
                                Ordering::Equal
                            }
                            Err(error) => {
                                failure = Some(error);
                                Ordering::Equal
                            }
                        });
                        if let Some(error) = failure {
                            return Err(error);
                        }
                        Ok(values)
                    }
                    let (ac, bc) = (canonical(ac, depth)?, canonical(bc, depth)?);
                    let mut order = Ordering::Equal;
                    for (a, b) in ac.iter().zip(&bc) {
                        order = compare_at(a, b, depth + 1, false)?
                            .ok_or(StructuralError::InvalidValue)?;
                        if order != Ordering::Equal {
                            break;
                        }
                    }
                    if order == Ordering::Equal {
                        order = ac.len().cmp(&bc.len());
                    }
                    if order != Ordering::Equal {
                        return Ok(Some(order));
                    }
                    continue;
                }
                if let (Some(ac), Some(bc)) = (children(a), children(b)) {
                    let ord = identity(a, b)?;
                    if ord != Ordering::Equal {
                        return Ok(Some(ord));
                    }
                    work.push(Work::Length(ac.len(), bc.len()));
                    for (a, b) in ac.iter().zip(bc).rev() {
                        work.push(Work::Pair(
                            a.as_ref().ok_or(StructuralError::InvalidValue)?,
                            b.as_ref().ok_or(StructuralError::InvalidValue)?,
                            depth + 1,
                        ));
                    }
                } else {
                    let result = scalar::binary(BinaryOp::Eq, a.clone(), b.clone())
                        .map_err(|_| StructuralError::InvalidValue)?;
                    if result == Value::Bool(true) {
                        continue;
                    }
                    let result = scalar::binary(BinaryOp::Lt, a.clone(), b.clone())
                        .map_err(|_| StructuralError::InvalidValue)?;
                    if result == Value::Bool(true) {
                        return Ok(Some(Ordering::Less));
                    }
                    let result = scalar::binary(BinaryOp::Gt, a.clone(), b.clone())
                        .map_err(|_| StructuralError::InvalidValue)?;
                    if result == Value::Bool(true) {
                        return Ok(Some(Ordering::Greater));
                    }
                    return Ok(None);
                }
            }
        }
    }
    Ok(Some(Ordering::Equal))
}
pub fn binary(op: BinaryOp, a: &Value, b: &Value) -> Result<Value, StructuralError> {
    let order = compare_at(a, b, 0, matches!(op, BinaryOp::Eq | BinaryOp::Ne))?;
    use BinaryOp::*;
    Ok(Value::Bool(match op {
        Eq => order == Some(Ordering::Equal),
        Ne => order != Some(Ordering::Equal),
        Lt => order == Some(Ordering::Less),
        Le => matches!(order, Some(Ordering::Less | Ordering::Equal)),
        Gt => order == Some(Ordering::Greater),
        Ge => matches!(order, Some(Ordering::Greater | Ordering::Equal)),
        _ => return Err(StructuralError::InvalidValue),
    }))
}
/// Stable length-framed encoding, using SipHash-1-3. Equal keys hash identically.
pub fn hash(value: &Value, seed: u64) -> Result<u64, StructuralError> {
    Ok(hirsel_kernels::siphash::siphash13(
        &encoding(value, 0)?,
        seed,
        0,
    ))
}
fn encoding(value: &Value, depth: usize) -> Result<Vec<u8>, StructuralError> {
    enum Work<'a> {
        Enter(&'a Value, usize),
        Finish(Vec<u8>, usize, bool),
    }
    let mut work = vec![Work::Enter(value, depth)];
    let mut output: Vec<Vec<u8>> = Vec::new();
    while let Some(item) = work.pop() {
        match item {
            Work::Finish(mut prefix, count, sort) => {
                let start = output.len() - count;
                let mut children = output.split_off(start);
                if sort {
                    children.sort();
                }
                for child in children {
                    framed(&mut prefix, &child);
                }
                output.push(prefix);
            }
            Work::Enter(v, d) => {
                if d > MAX_DEPTH {
                    return Err(StructuralError::Depth);
                }
                let tag = match v {
                    Value::Unit => 0,
                    Value::Bool(_) => 1,
                    Value::Integer(_) => 2,
                    Value::String(_) => 3,
                    Value::Bytes(_) => 4,
                    Value::Tuple(_) => 5,
                    Value::Variant { .. } => 6,
                    Value::Record { .. } => 7,
                    Value::Distinct { .. } => 8,
                    Value::Box(_) => 9,
                    Value::Sequence { .. } => 10,
                    Value::Map(_) => 11,
                    Value::Set(_) => 12,
                    _ => return Err(StructuralError::InvalidValue),
                };
                let mut bytes = vec![tag];
                match v {
                    Value::Bool(b) => bytes.push(u8::from(*b)),
                    Value::Integer(i) => {
                        bytes.extend_from_slice(&i.width().to_le_bytes());
                        bytes.push(u8::from(i.is_signed()));
                        bytes.extend_from_slice(&i.bits().to_le_bytes());
                    }
                    Value::String(s) => framed(&mut bytes, s.as_bytes()),
                    Value::Bytes(b) => framed(&mut bytes, b),
                    Value::Variant { variant, .. } => {
                        bytes.extend_from_slice(&variant.to_le_bytes())
                    }
                    Value::Record { definition, .. } | Value::Distinct { definition, .. } => {
                        framed(&mut bytes, definition.0.package.name.as_bytes());
                        framed(&mut bytes, definition.0.package.version.as_bytes());
                        bytes.extend_from_slice(&(definition.0.modules.len() as u64).to_le_bytes());
                        for m in &definition.0.modules {
                            framed(&mut bytes, m.as_bytes());
                        }
                        framed(&mut bytes, definition.0.name.as_bytes());
                    }
                    _ => (),
                }
                if let Some(fields) = children(v) {
                    bytes.extend_from_slice(&(fields.len() as u64).to_le_bytes());
                    work.push(Work::Finish(
                        bytes,
                        fields.len(),
                        matches!(v, Value::Set(_)),
                    ));
                    for f in fields.iter().rev() {
                        work.push(Work::Enter(
                            f.as_ref().ok_or(StructuralError::InvalidValue)?,
                            d + 1,
                        ));
                    }
                } else {
                    output.push(bytes);
                }
            }
        }
    }
    output.pop().ok_or(StructuralError::InvalidValue)
}
fn framed(out: &mut Vec<u8>, b: &[u8]) {
    out.extend_from_slice(&(b.len() as u64).to_le_bytes());
    out.extend_from_slice(b);
}
impl From<ScalarError> for StructuralError {
    fn from(_: ScalarError) -> Self {
        Self::InvalidValue
    }
}
