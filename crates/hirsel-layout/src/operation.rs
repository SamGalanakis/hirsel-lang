//! Concrete operation actuals and independently supplied registry declarations.
//! Only the type structure read by native layout/ABI instantiation crosses LIR.
use crate::layout::{LayoutBuilder, Owners};
use hirsel_core_contract::OperationId;
use hirsel_intern::Symbol;
use hirsel_lir::abi_types::{CallMultiplicity, CallableShape, FnAbi};
use hirsel_lir::layout::{LayoutError, MAX_DEPTH};
use hirsel_lir::operation::{NativeType, OperationArgument, OperationDeclarations};
use hirsel_syntax::DefId;
use hirsel_types::*;
use std::collections::BTreeMap;
use std::sync::Arc;

pub fn collect_arguments(
    types: &TypeInterner,
    arguments: &[GenericArgument],
) -> Result<Vec<OperationArgument>, LayoutError> {
    arguments
        .iter()
        .map(|a| from_argument(types, a, 0))
        .collect()
}

fn from_argument(
    types: &TypeInterner,
    argument: &GenericArgument,
    depth: u32,
) -> Result<OperationArgument, LayoutError> {
    Ok(match argument {
        GenericArgument::Type(ty) => OperationArgument::Type(from_type(types, *ty, depth)?),
        GenericArgument::Size(Size::Constant(n)) => OperationArgument::Size(*n),
        GenericArgument::Size(Size::Parameter(_)) => return Err(LayoutError::Incomplete),
    })
}

fn intern_argument(
    actual: &OperationArgument,
    types: &TypeInterner,
    depth: u32,
) -> Result<GenericArgument, LayoutError> {
    Ok(match actual {
        OperationArgument::Type(ty) => GenericArgument::Type(intern_native(ty, types, depth)?),
        OperationArgument::Size(n) => GenericArgument::Size(Size::Constant(*n)),
    })
}

fn from_type(types: &TypeInterner, ty: TypeId, depth: u32) -> Result<NativeType, LayoutError> {
    if depth > MAX_DEPTH {
        return Err(LayoutError::InstanceLimit);
    }
    let row = types.get(ty).ok_or(LayoutError::MissingType)?;
    let child = |ty| from_type(types, ty, depth + 1);
    let args = |arguments: &[GenericArgument]| {
        arguments
            .iter()
            .map(|a| from_argument(types, a, depth + 1))
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
        } => NativeType::Named(definition.path().clone(), args(arguments)?),
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

pub(crate) fn intern_native(
    actual: &NativeType,
    types: &TypeInterner,
    depth: u32,
) -> Result<TypeId, LayoutError> {
    if depth > MAX_DEPTH {
        return Err(LayoutError::InstanceLimit);
    }
    let children = |items: &[NativeType], types: &TypeInterner| {
        items
            .iter()
            .map(|t| intern_native(t, types, depth + 1))
            .collect::<Result<Vec<_>, _>>()
    };
    let args = |items: &[OperationArgument], types: &TypeInterner| {
        items
            .iter()
            .map(|a| intern_argument(a, types, depth + 1))
            .collect::<Result<Vec<_>, _>>()
    };
    let kind = match actual {
        NativeType::Primitive(p) => TypeKind::Primitive(primitive_from_native(*p)),
        NativeType::Tuple(items) => TypeKind::Tuple(children(items, types)?.into()),
        NativeType::Named(definition, arguments) => TypeKind::Named {
            definition: DefId::intern(definition),
            arguments: args(arguments, types)?.into(),
        },
        NativeType::Record(fields) => TypeKind::Record(
            fields
                .iter()
                .enumerate()
                .map(|(i, (growable, t))| {
                    Ok(Field {
                        name: Symbol::intern(&i.to_string()),
                        growable: *growable,
                        ty: intern_native(t, types, depth + 1)?,
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
                        name: Symbol::intern(&i.to_string()),
                        fields: children(fields, types)?
                            .into_iter()
                            .map(|t| (None, t))
                            .collect(),
                    })
                })
                .collect::<Result<Vec<_>, LayoutError>>()?
                .into(),
        ),
        NativeType::Function(_) => {
            let unit = types.intern(Type::unit());
            TypeKind::Function(empty_function(unit))
        }
        NativeType::Box(inner) => TypeKind::Box(intern_native(inner, types, depth + 1)?),
        NativeType::Builtin(kind, arguments) => TypeKind::Builtin {
            kind: builtin_from_native(*kind),
            arguments: args(arguments, types)?.into(),
        },
    };
    let mut row = Type {
        kind,
        ..Type::unit()
    };
    if let NativeType::Function(CallMultiplicity::Once) = actual {
        row.modes.multiplicity = Multiplicity::Once;
    }
    Ok(types.intern(row))
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
#[derive(Clone, Debug)]
pub struct OperationRegistry {
    pub seal_sources: BTreeMap<
        (hirsel_syntax::DefPath, hirsel_syntax::ContentHash),
        hirsel_lir::sealed_payload_source::SourceSeal,
    >,
    pub receipt_sources: BTreeMap<
        (hirsel_syntax::DefPath, hirsel_syntax::ContentHash),
        hirsel_lir::sealed_payload_source::ReceiptSource,
    >,
    pub pretransfer_adapters: Vec<hirsel_lir::pretransfer::AdapterReference>,
    /// Actuals come from the independent materialized primitive plan.
    pub pretransfer_actuals: Vec<(
        hirsel_lir::ir::Symbol,
        hirsel_lir::FnAbiId,
        OperationId,
        Vec<OperationArgument>,
    )>,
    pub invocation_bodies: Vec<hirsel_lir::invocations::ReferenceBody>,
    pub invocations:
        BTreeMap<hirsel_syntax::DefPath, Vec<hirsel_lir::invocations::ReferenceInvocation>>,
    /// The session interner the signatures' rows live in.
    pub types: Arc<TypeInterner>,
    pub signatures: BTreeMap<OperationId, SignatureRef>,
    pub type_declarations: BTreeMap<DefId, SignatureRef>,
    pub owners: Owners,
}

impl OperationDeclarations for OperationRegistry {
    fn receipt_source(
        &self,
        definition: &hirsel_syntax::DefPath,
        instance: hirsel_syntax::ContentHash,
    ) -> Option<hirsel_lir::sealed_payload_source::ReceiptSource> {
        self.receipt_sources
            .get(&(definition.clone(), instance))
            .cloned()
    }
    fn seal_source_witness(
        &self,
        definition: &hirsel_syntax::DefPath,
        instance: hirsel_syntax::ContentHash,
    ) -> Option<hirsel_lir::sealed_payload_source::SealSourceWitness> {
        self.seal_sources
            .get(&(definition.clone(), instance))
            .map(|source| source.witness.clone())
    }
    fn seal_events(
        &self,
        definition: &hirsel_syntax::DefPath,
        instance: hirsel_syntax::ContentHash,
    ) -> Option<Vec<hirsel_lir::sealed_payload_source::SourceEvent>> {
        self.seal_sources
            .get(&(definition.clone(), instance))
            .map(|source| source.events.clone())
    }
    fn seal_payloads(
        &self,
        definition: &hirsel_syntax::DefPath,
        instance: hirsel_syntax::ContentHash,
    ) -> Option<Vec<hirsel_lir::sealed_payload_source::SourcePayload>> {
        self.seal_sources
            .get(&(definition.clone(), instance))
            .map(|source| source.payloads.clone())
    }
    fn seal_descriptor_for_source(
        &self,
        definition: &hirsel_syntax::DefPath,
        instance: hirsel_syntax::ContentHash,
        root: &NativeType,
        table: &hirsel_lir::LayoutTable,
        target: &hirsel_lir::TargetSpec,
    ) -> Result<hirsel_lir::sealed::SealDescriptor, LayoutError> {
        let selected = self
            .seal_sources
            .get(&(definition.clone(), instance))
            .is_some_and(|source| {
                source.events.iter().any(|event| &event.root_type == root)
                    || source.payloads.iter().any(|payload| {
                        &payload.root_type == root
                            || payload.recipe.inputs.iter().any(|input| &input.ty == root)
                    })
            })
            || self
                .receipt_sources
                .get(&(definition.clone(), instance))
                .is_some_and(|source| {
                    source.profiles.iter().any(|profile| {
                        &profile.profile.root.ty == root && profile.profile.functions.is_empty()
                    })
                });
        if !selected {
            return Err(LayoutError::Invalid("sealed source is not selected".into()));
        }
        self.seal_descriptor(root, table, target)
    }

    fn pretransfer_adapters(&self) -> &[hirsel_lir::pretransfer::AdapterReference] {
        &self.pretransfer_adapters
    }

    fn pretransfer_abi(
        &self,
        reference: &hirsel_lir::pretransfer::AdapterReference,
        target: &hirsel_lir::TargetSpec,
    ) -> Option<(FnAbi, hirsel_lir::LayoutTable)> {
        if hirsel_lir::pretransfer::primitive(&reference.adapter.definition).as_ref()
            != Some(&reference.primitive.definition)
        {
            return None;
        }
        let mut matches = self
            .pretransfer_actuals
            .iter()
            .filter(|(adapter, _, _, _)| adapter == &reference.adapter);
        let (_, abi, operation, arguments) = matches.next()?;
        if matches.next().is_some()
            || *abi != reference.abi
            || operation.definition != reference.primitive.definition
        {
            return None;
        }
        self.derive_abi(operation, arguments, target, true).ok()
    }

    fn invocation_body(
        &self,
        definition: &hirsel_syntax::DefPath,
        instance: hirsel_syntax::ContentHash,
    ) -> Option<u32> {
        let mut selected = self
            .invocation_bodies
            .iter()
            .filter(|r| &r.definition == definition && r.instance == instance);
        let body = selected.next()?.body;
        selected.next().is_none().then_some(body)
    }
    fn requires_structural_reference(&self) -> bool {
        !self.invocations.is_empty()
    }

    fn seal_descriptor(
        &self,
        source: &NativeType,
        table: &hirsel_lir::LayoutTable,
        target: &hirsel_lir::TargetSpec,
    ) -> Result<hirsel_lir::sealed::SealDescriptor, LayoutError> {
        crate::sealed::derive(self, source, table, target)
    }

    fn structural_access(
        &self,
        source: &NativeType,
        table: &hirsel_lir::LayoutTable,
        target: &hirsel_lir::TargetSpec,
    ) -> Result<hirsel_lir::structural_access::AccessPlan, LayoutError> {
        let root = intern_native(source, &self.types, 0)?;
        crate::structural_access::derive(
            &self.types,
            root,
            &mut |definition| Ok(self.type_declarations.get(&definition).cloned()),
            &self.owners,
            target,
            table,
        )
    }

    fn requires_invocation_reference(&self) -> bool {
        !self.invocations.is_empty()
    }

    fn invocations(
        &self,
        definition: &hirsel_syntax::DefPath,
    ) -> Option<&[hirsel_lir::invocations::ReferenceInvocation]> {
        self.invocations.get(definition).map(Vec::as_slice)
    }

    /// Derive from the named declaration, never from the call's ABI or operands.
    fn derive(
        &self,
        operation: &OperationId,
        arguments: &[OperationArgument],
        target: &hirsel_lir::TargetSpec,
    ) -> Result<(FnAbi, hirsel_lir::LayoutTable), LayoutError> {
        self.derive_abi(operation, arguments, target, false)
    }
}

impl OperationRegistry {
    fn derive_abi(
        &self,
        operation: &OperationId,
        arguments: &[OperationArgument],
        target: &hirsel_lir::TargetSpec,
        materialized: bool,
    ) -> Result<(FnAbi, hirsel_lir::LayoutTable), LayoutError> {
        let signature = self
            .signatures
            .get(operation)
            .ok_or_else(|| LayoutError::Invalid("missing operation declaration".into()))?;
        if signature.definition.path() != &operation.definition
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
        let types = &*self.types;
        let arguments = arguments
            .iter()
            .map(|a| intern_argument(a, types, 0))
            .collect::<Result<Vec<_>, _>>()?;
        let authorities = vec![AuthoritySummary::Unknown; arguments.len()];
        let substitution = hirsel_types::Substitution {
            binder: Some(GenericBinder::Bound(0)),
            arguments: &arguments,
            authorities: &authorities,
            ..Default::default()
        };
        let memo = TypeMemo::new();
        let one = |ty| {
            substitute(types, &memo, ty, &substitution)
                .map_err(|e| LayoutError::Invalid(format!("operation substitution: {e:?}")))
        };
        let mut parameters = Vec::new();
        for parameter in function.parameters.iter() {
            let mut p = parameter.clone();
            p.ty = one(p.ty)?;
            parameters.push(p);
        }
        let result = one(function.result)?;
        // Match the source-independent library request instantiation in
        // hirsel-rc/src/instance.rs::instantiate_function. Runtime primitive
        // derivation retains its existing ABI namespace.
        let callable = if materialized {
            let error = function.error.map(one).transpose()?;
            types.intern(Type {
                modes: Modes::global(),
                ..Type::with_kind(TypeKind::Function(FunctionType {
                    state_promises: function.state_promises.clone(),
                    generics: Vec::new().into(),
                    parameters: parameters.into(),
                    result,
                    error,
                    allocation: function.allocation,
                    authority: AuthoritySummary::Unknown,
                    uses: function.uses.clone(),
                    contracts: Vec::new().into(),
                    incomplete: false,
                    foreign: function.foreign.clone(),
                }))
            })
        } else {
            types.intern(Type {
                kind: TypeKind::Function(FunctionType {
                    parameters: parameters.into(),
                    foreign: function.foreign.clone(),
                    ..empty_function(result)
                }),
                ..Type::unit()
            })
        };
        let mut builder = LayoutBuilder::new(target, &self.owners);
        let abi = builder.fn_abi(types, callable, CallableShape::Definition)?;
        let abi = if materialized {
            let parameters = builder.table().fn_abis[abi.0 as usize].nonretaining.clone();
            builder.with_scoped_contract(abi, parameters, Vec::new())?
        } else {
            abi
        };
        let abi = if materialized && signature.definition.path().is_core("region.with") {
            let TypeKind::Function(function) = &types.row(callable).kind else {
                unreachable!()
            };
            let nested =
                builder.fn_abi(types, function.parameters[1].ty, CallableShape::Closure)?;
            let nested = builder.with_scoped_contract(
                nested,
                vec![hirsel_lir::abi_types::ParameterPath {
                    parameter: 0,
                    fields: Vec::new(),
                }],
                Vec::new(),
            )?;
            builder.with_scoped_entry(
                abi,
                hirsel_lir::abi_types::ParameterPath {
                    parameter: 1,
                    fields: Vec::new(),
                },
                nested,
            )?
        } else {
            abi
        };
        let abi = if materialized || signature.metadata.track_caller {
            builder.with_caller_site(abi)?
        } else {
            abi
        };
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
        Primitive::Char => hirsel_lir::operation::Primitive::Char,
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
        hirsel_lir::operation::Primitive::Char => Primitive::Char,
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
