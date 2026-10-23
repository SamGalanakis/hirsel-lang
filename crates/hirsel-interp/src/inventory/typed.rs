//! Type-directed heap occurrence charge. Placement receipts are transported by
//! the evaluator; neither a host pointer nor an absent receipt implies heap.
//! This walks values using the existing logical type resolver, not a second
//! declaration shape graph. Native shape/physical correspondence is separate.
use super::{Charge, LogicalBlockId};
use crate::{Error, Value, aggregate::RuntimeTypes, invalid};
use hirsel_type_analysis::logical::{Logical, LogicalNominal, Resolved, Scope, type_arguments};
use hirsel_types::{BuiltinType, Primitive, TypeId, TypeKind};
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BlockPlacement {
    Heap,
    Arena { arena: u64, block: LogicalBlockId },
}

/// Value-field paths follow active payload ordinals, including sequence indices.
/// Captured type rows are the actual closed environment types, never callable
/// parameter/result types. These receipts record placement, not authorization:
/// grant, resource, locality and no-escape proofs remain independent V duties.
#[derive(Clone, Debug, Default)]
pub struct PlacementReceipt {
    pub blocks: BTreeMap<Vec<u32>, BlockPlacement>,
    pub captures: BTreeMap<Vec<u32>, Vec<TypeId>>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ChargeRoot {
    Value,
    /// Enumerate this retained arena allocation's outgoing heap occurrences,
    /// excluding its already charged construction and its arena child entries.
    RetainedPayload,
}

/// Additional heap charge, stopping at designated-arena blocks and Shared
/// boundaries. Equal or aliased occurrences are deliberately never deduplicated.
pub fn heap_charge(
    runtime: &RuntimeTypes,
    ty: TypeId,
    value: &Value,
    arena: u64,
    receipt: &PlacementReceipt,
    root: ChargeRoot,
) -> Result<Charge, Error> {
    heap_charge_with(
        runtime,
        ty,
        value,
        arena,
        root,
        |path| {
            receipt
                .blocks
                .get(path)
                .copied()
                .ok_or_else(|| invalid("missing sealed block placement"))
        },
        |path| {
            receipt
                .captures
                .get(path)
                .cloned()
                .ok_or_else(|| invalid("missing sealed capture types"))
        },
    )
}

/// Callback adapter for evaluator-owned placement metadata. Lookup is fallible:
/// missing/unknown placement must never silently become heap placement.
pub fn heap_charge_with(
    runtime: &RuntimeTypes,
    ty: TypeId,
    value: &Value,
    arena: u64,
    root: ChargeRoot,
    mut placement: impl FnMut(&[u32]) -> Result<BlockPlacement, Error>,
    mut capture_types: impl FnMut(&[u32]) -> Result<Vec<TypeId>, Error>,
) -> Result<Charge, Error> {
    let resolve = |definition: hirsel_syntax::DefId| {
        runtime
            .get(definition.path())
            .map(|n| match n.representation {
                Some(shape) => LogicalNominal::Representation { shape },
                None => LogicalNominal::Opaque,
            })
    };
    let logical = Logical::new(&resolve);
    let mut work = vec![(value, ty, Scope::root(&runtime.types), Vec::new())];
    let mut total = Charge::ZERO;
    while let Some((value, ty, scope, path)) = work.pop() {
        if matches!(
            value,
            Value::Resource { .. } | Value::ResourceVariant { .. } | Value::Mutex(_)
        ) {
            return Err(invalid("a resource in sealed storage"));
        }
        let Resolved::Structural(row, scope) = logical
            .resolve(ty, &scope)
            .map_err(|e| invalid(format!("unresolved sealed storage type: {e:?}")))?
        else {
            return Err(invalid(
                "opaque sealed storage lacks a complete held representation",
            ));
        };
        let size = |ty| {
            logical
                .checked_size(ty, &scope)
                .map(|size| size.map_or(Charge::Overflow, Charge::Bytes))
                .map_err(|e| invalid(format!("unresolved sealed logical size: {e:?}")))
        };
        let mismatch = || invalid("sealed storage value does not match its logical type");
        // The block's own logical extent. None means inline storage, including
        // independent Shared handles whose frozen graph is not an owned edge.
        let own = match (&row.kind, value) {
            (
                TypeKind::Primitive(Primitive::Bytes),
                Value::Bytes(bytes) | Value::ArenaBytes { bytes, .. },
            ) => Some(Charge::Bytes(bytes.len() as u64)),
            (
                TypeKind::Primitive(Primitive::String),
                Value::String(text) | Value::ArenaString { text, .. },
            ) => Some(Charge::Bytes(text.len() as u64)),
            (TypeKind::Box(payload), Value::Box(_) | Value::ArenaBox { .. }) => {
                Some(size(*payload)?)
            }
            (
                TypeKind::Builtin {
                    kind: BuiltinType::List | BuiltinType::Array,
                    arguments,
                },
                Value::Sequence { elements, .. },
            ) => {
                let types = type_arguments(arguments);
                Some(size(*types.first().ok_or_else(mismatch)?)?.times(elements.len() as u64))
            }
            (
                TypeKind::Builtin {
                    kind: BuiltinType::Set,
                    arguments,
                },
                Value::Set { entries, .. },
            ) => {
                let types = type_arguments(arguments);
                Some(
                    size(*types.first().ok_or_else(mismatch)?)?
                        .times(entries.iter().flatten().count() as u64),
                )
            }
            (
                TypeKind::Builtin {
                    kind: BuiltinType::Map,
                    arguments,
                },
                Value::Map { entries, .. },
            ) => {
                let types = type_arguments(arguments);
                let [key, item, ..] = types.as_slice() else {
                    return Err(mismatch());
                };
                Some(
                    size(*key)?
                        .plus(size(*item)?)
                        .times(entries.iter().flatten().count() as u64),
                )
            }
            (
                TypeKind::Function(_),
                Value::Closure {
                    captures,
                    environment,
                    ..
                },
            ) => {
                let types = capture_types(&path)?;
                if types.len() != captures.len() || types.len() != environment.owned_captures.len()
                {
                    return Err(mismatch());
                }
                let mut bytes = Charge::ZERO;
                for (i, ty) in types.iter().enumerate() {
                    if environment.owned_captures[i] {
                        bytes = bytes.plus(
                            logical
                                .checked_size(*ty, &Scope::root(&runtime.types))
                                .map(|size| size.map_or(Charge::Overflow, Charge::Bytes))
                                .map_err(|e| {
                                    invalid(format!("unresolved sealed capture size: {e:?}"))
                                })?,
                        );
                    } else if captures[i].is_some() {
                        return Err(invalid("borrowed capture in sealed storage"));
                    }
                }
                Some(bytes)
            }
            _ => None,
        };
        if let Some(bytes) = own {
            let storage = placement(&path)?;
            if let Some(tag) = value.arena_storage() {
                let Some(block) = tag.logical_block else {
                    return Err(invalid("missing semantic block identity"));
                };
                if storage
                    != (BlockPlacement::Arena {
                        arena: tag.arena,
                        block,
                    })
                {
                    return Err(invalid("sealed block receipt disagrees with arena storage"));
                }
            }
            match storage {
                BlockPlacement::Arena { arena: owner, .. } if owner != arena => {
                    return Err(invalid("foreign arena in sealed storage"));
                }
                BlockPlacement::Arena { .. }
                    if path.is_empty() && root == ChargeRoot::RetainedPayload => {}
                BlockPlacement::Arena { .. } => continue,
                BlockPlacement::Heap if path.is_empty() && root == ChargeRoot::RetainedPayload => {
                    return Err(invalid("retained payload is not arena owned"));
                }
                BlockPlacement::Heap => total = total.plus(bytes),
            }
        } else if path.is_empty() && root == ChargeRoot::RetainedPayload {
            return Err(invalid("retained payload is not an allocated block"));
        }
        macro_rules! push {
            ($index:expr, $child:expr, $ty:expr, $scope:expr) => {{
                let mut child_path = path.clone();
                child_path.push(
                    u32::try_from($index).map_err(|_| invalid("sealed field ordinal overflow"))?,
                );
                work.push(($child, $ty, $scope, child_path));
            }};
        }
        match (&row.kind, value) {
            (TypeKind::Primitive(Primitive::Bytes), Value::Bytes(_) | Value::ArenaBytes { .. })
            | (
                TypeKind::Primitive(Primitive::String),
                Value::String(_) | Value::ArenaString { .. },
            ) => {}
            (TypeKind::Primitive(p), value) if scalar_matches(*p, value) => {}
            (TypeKind::Box(payload), Value::Box(fields) | Value::ArenaBox { fields, .. }) => {
                let [Some(child)] = fields.as_slice() else {
                    return Err(mismatch());
                };
                push!(0, child, *payload, scope.clone());
            }
            (TypeKind::Tuple(types), Value::Tuple(fields)) if types.len() == fields.len() => {
                for (i, (ty, field)) in types.iter().zip(fields).enumerate() {
                    push!(i, field.as_ref().ok_or_else(mismatch)?, *ty, scope.clone());
                }
            }
            (TypeKind::Tuple(types), Value::Unit) if types.is_empty() => {}
            (
                TypeKind::Record(types),
                Value::Record { fields, .. } | Value::Distinct { fields, .. },
            ) if types.len() == fields.len() => {
                for (i, (field, value)) in types.iter().zip(fields).enumerate() {
                    push!(
                        i,
                        value.as_ref().ok_or_else(mismatch)?,
                        field.ty,
                        scope.clone()
                    );
                }
            }
            (TypeKind::Variant(types), Value::Variant { variant, fields }) => {
                let variant = types.get(*variant as usize).ok_or_else(mismatch)?;
                if variant.fields.len() != fields.len() {
                    return Err(mismatch());
                }
                for (i, ((_, ty), field)) in variant.fields.iter().zip(fields).enumerate() {
                    push!(i, field.as_ref().ok_or_else(mismatch)?, *ty, scope.clone());
                }
            }
            (
                TypeKind::Builtin {
                    kind: BuiltinType::Shared,
                    ..
                },
                Value::Shared(_),
            ) => {}
            (
                TypeKind::Builtin {
                    kind: BuiltinType::Packed,
                    arguments,
                },
                value,
            ) => {
                let types = type_arguments(arguments);
                // Packed does not add a value field or a storage boundary.
                work.push((value, *types.first().ok_or_else(mismatch)?, scope, path));
            }
            (
                TypeKind::Builtin {
                    kind:
                        BuiltinType::List | BuiltinType::Array | BuiltinType::Inline | BuiltinType::Simd,
                    arguments,
                },
                Value::Sequence { elements, .. },
            )
            | (
                TypeKind::Builtin {
                    kind: BuiltinType::Set,
                    arguments,
                },
                Value::Set {
                    entries: elements, ..
                },
            ) => {
                let types = type_arguments(arguments);
                let ty = *types.first().ok_or_else(mismatch)?;
                for (i, value) in elements.iter().enumerate() {
                    if let Some(value) = value {
                        push!(i, value, ty, scope.clone());
                    }
                }
            }
            (
                TypeKind::Builtin {
                    kind: BuiltinType::Map,
                    arguments,
                },
                Value::Map { entries, .. },
            ) => {
                let types = type_arguments(arguments);
                let [key, item, ..] = types.as_slice() else {
                    return Err(mismatch());
                };
                for (i, entry) in entries.iter().enumerate() {
                    let Some(entry) = entry else {
                        continue;
                    };
                    let Value::Tuple(fields) = entry else {
                        return Err(mismatch());
                    };
                    let [Some(k), Some(v)] = fields.as_slice() else {
                        return Err(mismatch());
                    };
                    let mut entry_path = path.clone();
                    entry_path.push(
                        u32::try_from(i).map_err(|_| invalid("sealed entry ordinal overflow"))?,
                    );
                    for (index, value, ty) in [(0, k, *key), (1, v, *item)] {
                        let mut field_path = entry_path.clone();
                        field_path.push(index);
                        work.push((value, ty, scope.clone(), field_path));
                    }
                }
            }
            (TypeKind::Function(_), Value::FunctionReference(_)) => {}
            (
                TypeKind::Function(_),
                Value::Closure {
                    captures,
                    environment,
                    ..
                },
            ) => {
                let types = capture_types(&path)?;
                for (i, (value, ty)) in captures.iter().zip(&types).enumerate() {
                    if environment.owned_captures[i] {
                        push!(
                            i,
                            value.as_ref().ok_or_else(mismatch)?,
                            *ty,
                            Scope::root(&runtime.types)
                        );
                    }
                }
            }
            _ => return Err(mismatch()),
        }
    }
    Ok(total)
}

fn scalar_matches(ty: Primitive, value: &Value) -> bool {
    match (ty, value) {
        (Primitive::Bool, Value::Bool(_))
        | (Primitive::Char, Value::Char(_))
        | (Primitive::F32, Value::F32(_))
        | (Primitive::F64, Value::F64(_)) => true,
        (ty, Value::Integer(integer)) => integer.kind() == ty,
        _ => false,
    }
}

/// Recover the explicit reference placement receipt from value-carried tags.
/// Heap variants are semantic heap values, not an inference from host pointers.
/// Arena tags without an enrolled semantic identity are incomplete evidence.
pub(crate) fn value_receipt(value: &Value) -> Result<PlacementReceipt, Error> {
    let mut receipt = PlacementReceipt::default();
    let mut pending = vec![(value, Vec::new())];
    while let Some((value, path)) = pending.pop() {
        if matches!(value, Value::Shared(_)) {
            continue;
        }
        if value.is_block() {
            let placement = match value.arena_storage() {
                Some(storage) => BlockPlacement::Arena {
                    arena: storage.arena,
                    block: storage
                        .logical_block
                        .ok_or_else(|| invalid("unenrolled arena allocation"))?,
                },
                None => BlockPlacement::Heap,
            };
            receipt.blocks.insert(path.clone(), placement);
        }
        if let Value::Closure {
            captures,
            environment,
            ..
        } = value
        {
            if environment.capture_types.len() != captures.len()
                || environment.owned_captures.len() != captures.len()
            {
                return Err(invalid("incomplete closed capture metadata"));
            }
            receipt
                .captures
                .insert(path.clone(), environment.capture_types.clone());
        }
        for (index, child) in value.fields().into_iter().flatten().enumerate() {
            if let Some(child) = child {
                let mut child_path = path.clone();
                child_path.push(
                    u32::try_from(index).map_err(|_| invalid("sealed field ordinal overflow"))?,
                );
                pending.push((child, child_path));
            }
        }
    }
    Ok(receipt)
}
