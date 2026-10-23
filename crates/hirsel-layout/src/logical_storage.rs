//! Storage side facts over G12's one shared logical shape derivation.
use hirsel_lir::logical_storage::{LogicalBytes, LogicalStorageGraph, StorageClass, StorageFact};
use hirsel_syntax::DefId;
use hirsel_type_analysis::logical::{Logical, LogicalNominal, Scope};
use hirsel_types::*;
use std::collections::BTreeMap;

pub fn derive(
    types: &TypeInterner,
    root: TypeId,
    lookup: &mut impl FnMut(DefId) -> Result<Option<SignatureRef>, QueryError>,
) -> Result<Option<LogicalStorageGraph>, QueryError> {
    let mut nominals = BTreeMap::new();
    let derived = crate::structural::derive_typed(types, root, &mut |definition| {
        let signature = lookup(definition)?;
        if let Some(signature) = &signature {
            if let SignatureKind::Type(declaration) = &signature.kind {
                nominals.insert(
                    definition,
                    match declaration.representation {
                        Some(shape) => LogicalNominal::Representation { shape },
                        None => LogicalNominal::Opaque,
                    },
                );
            }
        }
        Ok(signature)
    })?;
    let Some(derived) = derived else {
        return Ok(None);
    };
    let resolve = |definition| nominals.get(&definition).copied();
    let logical = Logical::new(&resolve);
    let scope = Scope::root(types);
    let mut facts = Vec::with_capacity(derived.node_types.len());
    for ty in derived.node_types {
        let Some(row) = types.get(ty) else {
            return Ok(None);
        };
        let class = match &row.kind {
            TypeKind::Primitive(Primitive::Bytes | Primitive::String) => StorageClass::Bytes,
            TypeKind::Primitive(_)
            | TypeKind::Record(_)
            | TypeKind::Tuple(_)
            | TypeKind::Variant(_) => StorageClass::Inline,
            TypeKind::Box(_) => StorageClass::Block,
            TypeKind::Named { .. } => StorageClass::Transparent,
            TypeKind::Builtin { kind, arguments } => match kind {
                BuiltinType::List | BuiltinType::Array | BuiltinType::Map | BuiltinType::Set => {
                    StorageClass::Block
                }
                BuiltinType::Shared => StorageClass::Shared,
                BuiltinType::Packed => StorageClass::Transparent,
                BuiltinType::Inline | BuiltinType::Simd => {
                    let Some(count) = arguments.iter().find_map(|a| match a {
                        GenericArgument::Size(Size::Constant(n)) => Some(*n),
                        _ => None,
                    }) else {
                        return Ok(None);
                    };
                    StorageClass::InlineSequence { count }
                }
                _ => return Ok(None),
            },
            TypeKind::Function(_) | TypeKind::Parameter(_) | TypeKind::Error => return Ok(None),
        };
        let Ok(bytes) = logical.checked_size(ty, &scope) else {
            return Ok(None);
        };
        facts.push(StorageFact {
            bytes: bytes.map_or(LogicalBytes::Overflow, LogicalBytes::Bytes),
            class,
        });
    }
    let graph = LogicalStorageGraph {
        shape: derived.graph,
        facts,
    };
    Ok(graph.check().is_ok().then_some(graph))
}

/// The candidate's graph and every side fact must agree with declarations.
/// A physical layout's shape or byte width cannot supply this evidence.
pub fn validate(
    candidate: &LogicalStorageGraph,
    types: &TypeInterner,
    root: TypeId,
    lookup: &mut impl FnMut(DefId) -> Result<Option<SignatureRef>, QueryError>,
) -> Result<bool, QueryError> {
    Ok(candidate.check().is_ok() && derive(types, root, lookup)?.as_ref() == Some(candidate))
}
