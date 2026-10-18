//! Concrete operation actuals and independently supplied registry declarations.
//! Only the type structure read by native layout/ABI instantiation crosses LIR.
use crate::layout::{LayoutBuilder, Owners};
use hirsel_core_contract::OperationId;
use hirsel_lir::abi_types::{CallMultiplicity, CallableShape, FnAbi};
use hirsel_lir::layout::{LayoutError, MAX_DEPTH};
use hirsel_lir::operation::{NativeType, OperationArgument, OperationDeclarations};
use hirsel_types::*;
use std::collections::BTreeMap;

pub fn collect_arguments(
    store: &TypeStore,
    arguments: &[GenericArgument],
) -> Result<Vec<OperationArgument>, LayoutError> {
    arguments
        .iter()
        .map(|a| from_argument(store, a, 0))
        .collect()
}

fn from_argument(
    store: &TypeStore,
    argument: &GenericArgument,
    depth: u32,
) -> Result<OperationArgument, LayoutError> {
    Ok(match argument {
        GenericArgument::Type(ty) => OperationArgument::Type(from_type(store, *ty, depth)?),
        GenericArgument::Size(Size::Constant(n)) => OperationArgument::Size(*n),
        GenericArgument::Size(Size::Parameter(_)) => return Err(LayoutError::Incomplete),
    })
}

fn import_argument(
    actual: &OperationArgument,
    store: &mut TypeStore,
    depth: u32,
) -> Result<GenericArgument, LayoutError> {
    Ok(match actual {
        OperationArgument::Type(ty) => GenericArgument::Type(import_type(ty, store, depth)?),
        OperationArgument::Size(n) => GenericArgument::Size(Size::Constant(*n)),
    })
}

fn from_type(store: &TypeStore, ty: TypeId, depth: u32) -> Result<NativeType, LayoutError> {
    if depth > MAX_DEPTH {
        return Err(LayoutError::InstanceLimit);
    }
    if store.id(ty.index()) != Some(ty) {
        return Err(LayoutError::MissingType);
    }
    let row = store.get(ty).ok_or(LayoutError::MissingType)?;
    let child = |ty| from_type(store, ty, depth + 1);
    let args = |arguments: &[GenericArgument]| {
        arguments
            .iter()
            .map(|a| from_argument(store, a, depth + 1))
            .collect::<Result<_, _>>()
    };
    Ok(match &row.kind {
        TypeKind::Primitive(p) => NativeType::Primitive(primitive_to_native(*p)),
        TypeKind::Tuple(items) => {
            NativeType::Tuple(items.iter().copied().map(child).collect::<Result<_, _>>()?)
        }
        TypeKind::Named {
            definition,
            arguments,
        } => NativeType::Named(definition.clone(), args(arguments)?),
        TypeKind::Record(fields) => NativeType::Record(
            fields
                .iter()
                .map(|f| Ok((f.growable, child(f.ty)?)))
                .collect::<Result<_, LayoutError>>()?,
        ),
        TypeKind::Variant(variants) => NativeType::Variant(
            variants
                .iter()
                .map(|v| v.fields.iter().map(|(_, ty)| child(*ty)).collect())
                .collect::<Result<_, _>>()?,
        ),
        TypeKind::Function(_) => NativeType::Function(match row.modes.multiplicity {
            Multiplicity::Many => CallMultiplicity::Many,
            Multiplicity::Once => CallMultiplicity::Once,
        }),
        TypeKind::Box(inner) => NativeType::Box(Box::new(child(*inner)?)),
        TypeKind::Builtin { kind, arguments } => {
            NativeType::Builtin(builtin_to_native(*kind), args(arguments)?)
        }
        TypeKind::Parameter(_) => return Err(LayoutError::Incomplete),
        TypeKind::Error => {
            return Err(LayoutError::Invalid("recovery operation actual".into()));
        }
    })
}

fn import_type(
    actual: &NativeType,
    store: &mut TypeStore,
    depth: u32,
) -> Result<TypeId, LayoutError> {
    if depth > MAX_DEPTH {
        return Err(LayoutError::InstanceLimit);
    }
    let children = |items: &[NativeType], store: &mut TypeStore| {
        items
            .iter()
            .map(|t| import_type(t, store, depth + 1))
            .collect::<Result<Vec<_>, _>>()
    };
    let args = |items: &[OperationArgument], store: &mut TypeStore| {
        items
            .iter()
            .map(|a| import_argument(a, store, depth + 1))
            .collect::<Result<Vec<_>, _>>()
    };
    let kind = match actual {
        NativeType::Primitive(p) => TypeKind::Primitive(primitive_from_native(*p)),
        NativeType::Tuple(items) => TypeKind::Tuple(children(items, store)?.into()),
        NativeType::Named(definition, arguments) => TypeKind::Named {
            definition: definition.clone(),
            arguments: args(arguments, store)?.into(),
        },
        NativeType::Record(fields) => TypeKind::Record(
            fields
                .iter()
                .enumerate()
                .map(|(i, (growable, t))| {
                    Ok(Field {
                        name: i.to_string(),
                        growable: *growable,
                        ty: import_type(t, store, depth + 1)?,
                    })
                })
                .collect::<Result<Vec<_>, LayoutError>>()?
                .into(),
        ),
        NativeType::Variant(variants) => TypeKind::Variant(
            variants
                .iter()
                .enumerate()
                .map(|(i, fields)| {
                    Ok(Variant {
                        name: i.to_string(),
                        fields: children(fields, store)?
                            .into_iter()
                            .map(|t| (None, t))
                            .collect(),
                    })
                })
                .collect::<Result<Vec<_>, LayoutError>>()?
                .into(),
        ),
        NativeType::Function(_) => {
            let unit = store.push(Type::unit());
            TypeKind::Function(empty_function(unit))
        }
        NativeType::Box(inner) => TypeKind::Box(import_type(inner, store, depth + 1)?),
        NativeType::Builtin(kind, arguments) => TypeKind::Builtin {
            kind: builtin_from_native(*kind),
            arguments: args(arguments, store)?.into(),
        },
    };
    let mut row = Type {
        kind,
        ..Type::unit()
    };
    if let NativeType::Function(CallMultiplicity::Once) = actual {
        row.modes.multiplicity = Multiplicity::Once;
    }
    Ok(store.push(row))
}

fn empty_function(result: TypeId) -> FunctionType {
    FunctionType {
        state_promises: Default::default(),
        generics: Default::default(),
        parameters: Default::default(),
        result,
        error: None,
        allocation: Allocation::MayAllocate,
        authority: AuthoritySummary::Unknown,
        uses: Uses::Unrestricted,
        contracts: Default::default(),
        incomplete: false,
        foreign: None,
    }
}

/// Trusted input, supplied alongside the manifest rather than inside a unit.
#[derive(Clone, Debug, Default)]
pub struct OperationRegistry {
    pub signatures: BTreeMap<OperationId, SignatureRef>,
    pub owners: Owners,
}

impl OperationDeclarations for OperationRegistry {
    /// Derive from the named declaration, never from the call's ABI or operands.
    fn derive(
        &self,
        operation: &OperationId,
        arguments: &[OperationArgument],
        target: &hirsel_lir::TargetSpec,
    ) -> Result<(FnAbi, hirsel_lir::LayoutTable), LayoutError> {
        let signature = self
            .signatures
            .get(operation)
            .ok_or_else(|| LayoutError::Invalid("missing operation declaration".into()))?;
        if signature.definition != operation.definition
            || signature.fingerprint != operation.declaration_digest
        {
            return Err(LayoutError::Invalid(
                "operation declaration identity".into(),
            ));
        }
        let SignatureKind::Function(function) = &signature.kind else {
            return Err(LayoutError::Invalid(
                "operation declaration is not callable".into(),
            ));
        };
        if arguments.len() != function.generics.len()
            || function.generics.iter().zip(arguments).any(|(p, a)| {
                !matches!(
                    (p, a),
                    (GenericParameter::Type { .. }, OperationArgument::Type(_))
                        | (GenericParameter::Size { .. }, OperationArgument::Size(_))
                )
            })
        {
            return Err(LayoutError::Invalid(
                "operation actual kinds or arity".into(),
            ));
        }
        let mut store = TypeStore::default();
        let arguments = arguments
            .iter()
            .map(|a| import_argument(a, &mut store, 0))
            .collect::<Result<Vec<_>, _>>()?;
        let authorities = vec![AuthoritySummary::Unknown; arguments.len()];
        let substitution = hirsel_types::Substitution {
            binder: Some(GenericBinder::Bound(0)),
            arguments: &arguments,
            authorities: &authorities,
            ..Default::default()
        };
        let one = |ty, store: &mut TypeStore| {
            substitute_type(&signature.types, ty, store, substitution)
                .map_err(|e| LayoutError::Invalid(format!("operation substitution: {e:?}")))
        };
        let mut parameters = Vec::new();
        for parameter in function.parameters.iter() {
            let mut p = parameter.clone();
            p.ty = one(p.ty, &mut store)?;
            parameters.push(p);
        }
        let result = one(function.result, &mut store)?;
        let callable = store.push(Type {
            kind: TypeKind::Function(FunctionType {
                parameters: parameters.into(),
                foreign: function.foreign.clone(),
                ..empty_function(result)
            }),
            ..Type::unit()
        });
        let mut builder = LayoutBuilder::new(target, &self.owners);
        let abi = builder.fn_abi(&store, callable, CallableShape::Definition)?;
        let abi = builder.table().fn_abis[abi.0 as usize].clone();
        Ok((abi, builder.finish()))
    }
}

fn primitive_to_native(value: Primitive) -> hirsel_lir::operation::Primitive {
    match value {
        Primitive::Bool => hirsel_lir::operation::Primitive::Bool,
        Primitive::U8 => hirsel_lir::operation::Primitive::U8,
        Primitive::U16 => hirsel_lir::operation::Primitive::U16,
        Primitive::U32 => hirsel_lir::operation::Primitive::U32,
        Primitive::U64 => hirsel_lir::operation::Primitive::U64,
        Primitive::U128 => hirsel_lir::operation::Primitive::U128,
        Primitive::I8 => hirsel_lir::operation::Primitive::I8,
        Primitive::I16 => hirsel_lir::operation::Primitive::I16,
        Primitive::I32 => hirsel_lir::operation::Primitive::I32,
        Primitive::I64 => hirsel_lir::operation::Primitive::I64,
        Primitive::I128 => hirsel_lir::operation::Primitive::I128,
        Primitive::F32 => hirsel_lir::operation::Primitive::F32,
        Primitive::F64 => hirsel_lir::operation::Primitive::F64,
        Primitive::String => hirsel_lir::operation::Primitive::String,
        Primitive::Bytes => hirsel_lir::operation::Primitive::Bytes,
        Primitive::Never => hirsel_lir::operation::Primitive::Never,
    }
}

fn primitive_from_native(value: hirsel_lir::operation::Primitive) -> Primitive {
    match value {
        hirsel_lir::operation::Primitive::Bool => Primitive::Bool,
        hirsel_lir::operation::Primitive::U8 => Primitive::U8,
        hirsel_lir::operation::Primitive::U16 => Primitive::U16,
        hirsel_lir::operation::Primitive::U32 => Primitive::U32,
        hirsel_lir::operation::Primitive::U64 => Primitive::U64,
        hirsel_lir::operation::Primitive::U128 => Primitive::U128,
        hirsel_lir::operation::Primitive::I8 => Primitive::I8,
        hirsel_lir::operation::Primitive::I16 => Primitive::I16,
        hirsel_lir::operation::Primitive::I32 => Primitive::I32,
        hirsel_lir::operation::Primitive::I64 => Primitive::I64,
        hirsel_lir::operation::Primitive::I128 => Primitive::I128,
        hirsel_lir::operation::Primitive::F32 => Primitive::F32,
        hirsel_lir::operation::Primitive::F64 => Primitive::F64,
        hirsel_lir::operation::Primitive::String => Primitive::String,
        hirsel_lir::operation::Primitive::Bytes => Primitive::Bytes,
        hirsel_lir::operation::Primitive::Never => Primitive::Never,
    }
}

fn builtin_to_native(value: BuiltinType) -> hirsel_lir::operation::BuiltinType {
    match value {
        BuiltinType::Array => hirsel_lir::operation::BuiltinType::Array,
        BuiltinType::List => hirsel_lir::operation::BuiltinType::List,
        BuiltinType::Map => hirsel_lir::operation::BuiltinType::Map,
        BuiltinType::Set => hirsel_lir::operation::BuiltinType::Set,
        BuiltinType::SortedMap => hirsel_lir::operation::BuiltinType::SortedMap,
        BuiltinType::Inline => hirsel_lir::operation::BuiltinType::Inline,
        BuiltinType::Simd => hirsel_lir::operation::BuiltinType::Simd,
        BuiltinType::Packed => hirsel_lir::operation::BuiltinType::Packed,
        BuiltinType::Shared => hirsel_lir::operation::BuiltinType::Shared,
        BuiltinType::Slots => hirsel_lir::operation::BuiltinType::Slots,
        BuiltinType::SlotId => hirsel_lir::operation::BuiltinType::SlotId,
        BuiltinType::Region => hirsel_lir::operation::BuiltinType::Region,
        BuiltinType::Iter => hirsel_lir::operation::BuiltinType::Iter,
        BuiltinType::Task => hirsel_lir::operation::BuiltinType::Task,
        BuiltinType::Sender => hirsel_lir::operation::BuiltinType::Sender,
        BuiltinType::Receiver => hirsel_lir::operation::BuiltinType::Receiver,
        BuiltinType::Mutex => hirsel_lir::operation::BuiltinType::Mutex,
        BuiltinType::Atomic => hirsel_lir::operation::BuiltinType::Atomic,
        BuiltinType::Ptr => hirsel_lir::operation::BuiltinType::Ptr,
        BuiltinType::FnPtr => hirsel_lir::operation::BuiltinType::FnPtr,
        BuiltinType::TypeShape => hirsel_lir::operation::BuiltinType::TypeShape,
    }
}

fn builtin_from_native(value: hirsel_lir::operation::BuiltinType) -> BuiltinType {
    match value {
        hirsel_lir::operation::BuiltinType::Array => BuiltinType::Array,
        hirsel_lir::operation::BuiltinType::List => BuiltinType::List,
        hirsel_lir::operation::BuiltinType::Map => BuiltinType::Map,
        hirsel_lir::operation::BuiltinType::Set => BuiltinType::Set,
        hirsel_lir::operation::BuiltinType::SortedMap => BuiltinType::SortedMap,
        hirsel_lir::operation::BuiltinType::Inline => BuiltinType::Inline,
        hirsel_lir::operation::BuiltinType::Simd => BuiltinType::Simd,
        hirsel_lir::operation::BuiltinType::Packed => BuiltinType::Packed,
        hirsel_lir::operation::BuiltinType::Shared => BuiltinType::Shared,
        hirsel_lir::operation::BuiltinType::Slots => BuiltinType::Slots,
        hirsel_lir::operation::BuiltinType::SlotId => BuiltinType::SlotId,
        hirsel_lir::operation::BuiltinType::Region => BuiltinType::Region,
        hirsel_lir::operation::BuiltinType::Iter => BuiltinType::Iter,
        hirsel_lir::operation::BuiltinType::Task => BuiltinType::Task,
        hirsel_lir::operation::BuiltinType::Sender => BuiltinType::Sender,
        hirsel_lir::operation::BuiltinType::Receiver => BuiltinType::Receiver,
        hirsel_lir::operation::BuiltinType::Mutex => BuiltinType::Mutex,
        hirsel_lir::operation::BuiltinType::Atomic => BuiltinType::Atomic,
        hirsel_lir::operation::BuiltinType::Ptr => BuiltinType::Ptr,
        hirsel_lir::operation::BuiltinType::FnPtr => BuiltinType::FnPtr,
        hirsel_lir::operation::BuiltinType::TypeShape => BuiltinType::TypeShape,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hirsel_codec::{from_text, to_text};
    #[test]
    fn operation_tags_keep_the_semantic_snapshot_bytes() {
        for tag in [
            "Bool", "U8", "U16", "U32", "U64", "U128", "I8", "I16", "I32", "I64", "I128", "F32",
            "F64", "String", "Bytes", "Never",
        ] {
            let text = format!("({tag})");
            let semantic = from_text::<Primitive>(&text).unwrap();
            let native = primitive_to_native(semantic);
            assert_eq!(to_text(&native), text);
            assert_eq!(primitive_from_native(native), semantic);
        }
        for tag in [
            "Array",
            "List",
            "Map",
            "Set",
            "SortedMap",
            "Inline",
            "Simd",
            "Packed",
            "Shared",
            "Slots",
            "SlotId",
            "Region",
            "Iter",
            "Task",
            "Sender",
            "Receiver",
            "Mutex",
            "Atomic",
            "Ptr",
            "FnPtr",
            "TypeShape",
        ] {
            let text = format!("({tag})");
            let semantic = from_text::<BuiltinType>(&text).unwrap();
            let native = builtin_to_native(semantic);
            assert_eq!(to_text(&native), text);
            assert_eq!(builtin_from_native(native), semantic);
        }
    }
}
