//! Independent TIR certificate for the scalar CPU mapper subset. The native
//! reference is hirsel-lir's ahead scalar_graph; this walk never consumes its plan.
use crate::{AsyncTirProvider, Error, Value};
use hirsel_tir::{Callee, Constant, Rvalue, Terminator};
use hirsel_types::{Primitive, TypeId, TypeInterner, TypeKind};
use std::collections::BTreeSet;

fn scalar(types: &TypeInterner, ty: TypeId) -> bool {
    types.get(ty).is_some_and(|ty| match &ty.kind {
        TypeKind::Primitive(p) => matches!(
            p,
            Primitive::Bool
                | Primitive::U8
                | Primitive::U16
                | Primitive::U32
                | Primitive::U64
                | Primitive::I8
                | Primitive::I16
                | Primitive::I32
                | Primitive::I64
        ),
        TypeKind::Tuple(fields) => fields.iter().all(|ty| scalar(types, *ty)),
        _ => false,
    })
}

fn pure(path: &hirsel_syntax::DefPath) -> bool {
    [
        "int.wrapping_add",
        "int.wrapping_sub",
        "int.wrapping_mul",
        "int.saturating_add",
        "int.saturating_sub",
        "int.leading_zeros",
        "int.trailing_zeros",
        "int.count_ones",
        "int.swap_bytes",
        "int.reverse_bits",
        "f32.bits",
        "f32.from_bits",
        "f64.bits",
        "f64.from_bits",
    ]
    .iter()
    .any(|name| path.is_core(name))
}

pub(crate) async fn certified(
    provider: &mut (impl AsyncTirProvider + ?Sized),
    types: &TypeInterner,
    mapper: &Value,
) -> Result<bool, Error> {
    let (path, body) = match mapper {
        Value::Closure {
            definition,
            body,
            environment,
            ..
        } => {
            if !environment
                .capture_types
                .iter()
                .all(|ty| scalar(types, *ty))
            {
                return Ok(false);
            }
            (definition.path().clone(), Some(*body))
        }
        Value::FunctionReference(reference) => (reference.definition.clone(), None),
        _ => return Ok(false),
    };
    let mut todo = vec![(path, body, false)];
    let mut active = BTreeSet::new();
    let mut checked = BTreeSet::new();
    let mut instructions = 0;
    while let Some((path, selected, leave)) = todo.pop() {
        let definition = match provider.definition(&path).await {
            Ok(definition) => definition,
            Err(hirsel_types::QueryError::NotImplemented) => return Ok(false),
            Err(error) => return Err(Error::Query(error)),
        };
        let body_id = selected.unwrap_or(definition.entry);
        let key = (path.clone(), body_id.0);
        if leave {
            active.remove(&key);
            checked.insert(key);
            continue;
        }
        if checked.contains(&key) {
            continue;
        }
        if !active.insert(key) {
            return Ok(false);
        }
        todo.push((path, Some(body_id), true));
        // Bound the same dimensions as the native scalar graph. This reference
        // admits a conservative subset, so unknown representation stays refused.
        if checked.len() + active.len() >= 64 {
            return Ok(false);
        }
        let Some(body) = definition.bodies.get(body_id.0 as usize) else {
            return Ok(false);
        };
        instructions += body
            .blocks
            .iter()
            .map(|block| block.instructions.len() + 1)
            .sum::<usize>();
        if instructions > 4096
            || !scalar(types, body.result)
            || !body.locals.iter().all(|local| scalar(types, local.ty))
        {
            return Ok(false);
        }
        if body.rvalues.iter().any(|value| {
            !matches!(
                value,
                Rvalue::Constant(
                    Constant::Unit
                        | Constant::Bool(_)
                        | Constant::Integer(_)
                        | Constant::F32(_)
                        | Constant::F64(_)
                ) | Rvalue::Use(_)
                    | Rvalue::Unary { .. }
                    | Rvalue::Binary { .. }
                    | Rvalue::Size(_)
                    | Rvalue::Construct {
                        constructor: hirsel_tir::Constructor::Tuple,
                        ..
                    }
            )
        }) {
            return Ok(false);
        }
        for block in &body.blocks {
            if let Terminator::Call { callee, .. } = &block.terminator {
                match callee {
                    Callee::Definition { path, .. } if pure(path) => (),
                    Callee::Definition { path, .. } if path.0.package.name != "core" => {
                        todo.push((path.clone(), None, false))
                    }
                    _ => return Ok(false),
                }
            }
        }
    }
    Ok(true)
}
