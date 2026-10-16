//! Concrete operation actuals and independently supplied registry declarations.
//! Only the type structure read by native layout/ABI instantiation crosses LIR.
use crate::abi_types::{CallMultiplicity, CallableShape, FnAbi};
use crate::layout::{LayoutBuilder, LayoutError, MAX_DEPTH, Owners};
use hirsel_codec::snapshot_enum;
use hirsel_core_contract::OperationId;
use hirsel_syntax::DefPath;
use hirsel_types::*;
use std::collections::BTreeMap;
use std::hash::Hash;

#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub enum OperationArgument {
    Type(NativeType),
    Size(u64),
}

/// The same erased axes as LayoutBuilder's type identity. Callable values need
/// only multiplicity: their invocation signature does not affect their layout.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum NativeType {
    Primitive(Primitive),
    Tuple(Vec<NativeType>),
    Named(DefPath, Vec<OperationArgument>),
    Record(Vec<(bool, NativeType)>),
    Variant(Vec<Vec<NativeType>>),
    Function(CallMultiplicity),
    Box(Box<NativeType>),
    Builtin(BuiltinType, Vec<OperationArgument>),
}

impl std::hash::Hash for NativeType {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        hirsel_codec::to_text(self).hash(state);
    }
}

impl OperationArgument {
    pub fn collect(
        store: &TypeStore,
        arguments: &[GenericArgument],
    ) -> Result<Vec<Self>, LayoutError> {
        arguments
            .iter()
            .map(|a| Self::from_argument(store, a, 0))
            .collect()
    }

    fn from_argument(
        store: &TypeStore,
        argument: &GenericArgument,
        depth: u32,
    ) -> Result<Self, LayoutError> {
        Ok(match argument {
            GenericArgument::Type(ty) => Self::Type(NativeType::from_type(store, *ty, depth)?),
            GenericArgument::Size(Size::Constant(n)) => Self::Size(*n),
            GenericArgument::Size(Size::Parameter(_)) => return Err(LayoutError::Incomplete),
        })
    }

    fn import(&self, store: &mut TypeStore, depth: u32) -> Result<GenericArgument, LayoutError> {
        Ok(match self {
            Self::Type(ty) => GenericArgument::Type(ty.import(store, depth)?),
            Self::Size(n) => GenericArgument::Size(Size::Constant(*n)),
        })
    }
}

impl NativeType {
    fn from_type(store: &TypeStore, ty: TypeId, depth: u32) -> Result<Self, LayoutError> {
        if depth > MAX_DEPTH {
            return Err(LayoutError::InstanceLimit);
        }
        if store.id(ty.index()) != Some(ty) {
            return Err(LayoutError::MissingType);
        }
        let row = store.get(ty).ok_or(LayoutError::MissingType)?;
        let child = |ty| Self::from_type(store, ty, depth + 1);
        let args = |arguments: &[GenericArgument]| {
            arguments
                .iter()
                .map(|a| OperationArgument::from_argument(store, a, depth + 1))
                .collect::<Result<_, _>>()
        };
        Ok(match &row.kind {
            TypeKind::Primitive(p) => Self::Primitive(*p),
            TypeKind::Tuple(items) => {
                Self::Tuple(items.iter().copied().map(child).collect::<Result<_, _>>()?)
            }
            TypeKind::Named {
                definition,
                arguments,
            } => Self::Named(definition.clone(), args(arguments)?),
            TypeKind::Record(fields) => Self::Record(
                fields
                    .iter()
                    .map(|f| Ok((f.growable, child(f.ty)?)))
                    .collect::<Result<_, LayoutError>>()?,
            ),
            TypeKind::Variant(variants) => Self::Variant(
                variants
                    .iter()
                    .map(|v| v.fields.iter().map(|(_, ty)| child(*ty)).collect())
                    .collect::<Result<_, _>>()?,
            ),
            TypeKind::Function(_) => Self::Function(match row.modes.multiplicity {
                Multiplicity::Many => CallMultiplicity::Many,
                Multiplicity::Once => CallMultiplicity::Once,
            }),
            TypeKind::Box(inner) => Self::Box(Box::new(child(*inner)?)),
            TypeKind::Builtin { kind, arguments } => Self::Builtin(*kind, args(arguments)?),
            TypeKind::Parameter(_) => return Err(LayoutError::Incomplete),
            TypeKind::Error => {
                return Err(LayoutError::Invalid("recovery operation actual".into()));
            }
        })
    }

    fn import(&self, store: &mut TypeStore, depth: u32) -> Result<TypeId, LayoutError> {
        if depth > MAX_DEPTH {
            return Err(LayoutError::InstanceLimit);
        }
        let children = |items: &[NativeType], store: &mut TypeStore| {
            items
                .iter()
                .map(|t| t.import(store, depth + 1))
                .collect::<Result<Vec<_>, _>>()
        };
        let args = |items: &[OperationArgument], store: &mut TypeStore| {
            items
                .iter()
                .map(|a| a.import(store, depth + 1))
                .collect::<Result<Vec<_>, _>>()
        };
        let kind = match self {
            Self::Primitive(p) => TypeKind::Primitive(*p),
            Self::Tuple(items) => TypeKind::Tuple(children(items, store)?.into()),
            Self::Named(definition, arguments) => TypeKind::Named {
                definition: definition.clone(),
                arguments: args(arguments, store)?.into(),
            },
            Self::Record(fields) => TypeKind::Record(
                fields
                    .iter()
                    .enumerate()
                    .map(|(i, (growable, t))| {
                        Ok(Field {
                            name: i.to_string(),
                            growable: *growable,
                            ty: t.import(store, depth + 1)?,
                        })
                    })
                    .collect::<Result<Vec<_>, LayoutError>>()?
                    .into(),
            ),
            Self::Variant(variants) => TypeKind::Variant(
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
            Self::Function(_) => {
                let unit = store.push(Type::unit());
                TypeKind::Function(empty_function(unit))
            }
            Self::Box(inner) => TypeKind::Box(inner.import(store, depth + 1)?),
            Self::Builtin(kind, arguments) => TypeKind::Builtin {
                kind: *kind,
                arguments: args(arguments, store)?.into(),
            },
        };
        let mut row = Type {
            kind,
            ..Type::unit()
        };
        if let Self::Function(CallMultiplicity::Once) = self {
            row.modes.multiplicity = Multiplicity::Once;
        }
        Ok(store.push(row))
    }
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

impl OperationRegistry {
    /// Derive from the named declaration, never from the call's ABI or operands.
    pub fn derive(
        &self,
        operation: &OperationId,
        arguments: &[OperationArgument],
        target: &crate::TargetSpec,
    ) -> Result<(FnAbi, crate::LayoutTable), LayoutError> {
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
            .map(|a| a.import(&mut store, 0))
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

/// Compare the complete ABI, including logical type identity, with table-local
/// layout indices compared as rooted graphs. Recursive block layouts terminate
/// through the visited pair set; arena numbers have no cross-table meaning.
pub(crate) fn matches_abi(
    expected: &FnAbi,
    expected_table: &crate::LayoutTable,
    actual: &FnAbi,
    actual_table: &crate::LayoutTable,
) -> bool {
    let mut expected = expected.clone();
    let mut pairs = Vec::new();
    for (e, a) in expected.parameters.iter_mut().zip(&actual.parameters) {
        pairs.push((e.layout, a.layout));
        e.layout = a.layout;
    }
    if let (Some(e), Some(a)) = (&mut expected.result, &actual.result) {
        pairs.push((e.layout, a.layout));
        e.layout = a.layout;
    }
    if expected != *actual {
        return false;
    }
    let mut seen = std::collections::BTreeSet::new();
    while let Some((e, a)) = pairs.pop() {
        if !seen.insert((e, a)) {
            continue;
        }
        let (Some(e), Some(a)) = (expected_table.layout(e), actual_table.layout(a)) else {
            return false;
        };
        let mut e = e.clone();
        let mut a = a.clone();
        // As in Layouts::same_scalar, an internal scalar and the same typed
        // primitive are interchangeable. Their allocation keys differ; all
        // physical facts still agree, and FnAbiKey binds logical type identity.
        if matches!(e.kind, crate::layout_types::LayoutKind::Scalar(_)) && e.kind == a.kind {
            e.key.instance = None;
            a.key.instance = None;
        }
        let mut e_refs = Vec::new();
        let mut a_refs = Vec::new();
        erase_layout_refs(&mut e, &mut e_refs);
        erase_layout_refs(&mut a, &mut a_refs);
        if e != a || e_refs.len() != a_refs.len() {
            return false;
        }
        pairs.extend(e_refs.into_iter().zip(a_refs));
    }
    true
}

fn erase_layout_refs(record: &mut crate::LayoutRecord, refs: &mut Vec<crate::LayoutId>) {
    use crate::layout_types::*;
    fn take(id: &mut LayoutId, refs: &mut Vec<LayoutId>) {
        refs.push(*id);
        *id = LayoutId(0);
    }
    fn columns(c: &mut SoaColumns, refs: &mut Vec<LayoutId>) {
        for c in &mut c.columns {
            take(&mut c.layout, refs);
        }
    }
    fn scan(steps: &mut [ScanStep], refs: &mut Vec<LayoutId>) {
        for step in steps {
            match step {
                ScanStep::Inline { layout, .. } => take(layout, refs),
                ScanStep::Sequence { element, .. } | ScanStep::Repeat { element, .. } => {
                    take(element, refs)
                }
                ScanStep::ActiveVariant { variants, .. } => {
                    for (_, steps) in variants {
                        scan(steps, refs);
                    }
                }
                ScanStep::Columns(c) => columns(c, refs),
                ScanStep::Counted { .. }
                | ScanStep::Uncounted { .. }
                | ScanStep::SliceOwner { .. }
                | ScanStep::Environment { .. }
                | ScanStep::Handle { .. } => {}
            }
        }
    }
    fn drop(steps: &mut [DropStep], refs: &mut Vec<LayoutId>) {
        for step in steps {
            match step {
                DropStep::Child { layout, .. } => take(layout, refs),
                DropStep::ElementsReverse { element, .. }
                | DropStep::RepeatReverse { element, .. } => take(element, refs),
                DropStep::ActiveVariant { variants, .. } => {
                    for (_, steps) in variants {
                        drop(steps, refs);
                    }
                }
                DropStep::ColumnsReverse(c) => columns(c, refs),
                DropStep::Finalize
                | DropStep::Release { .. }
                | DropStep::Environment { .. }
                | DropStep::CloseHandle { .. } => {}
            }
        }
    }
    match &mut record.kind {
        LayoutKind::Inline { element, .. }
        | LayoutKind::Simd { element, .. }
        | LayoutKind::Slice { element }
        | LayoutKind::Iterator { element }
        | LayoutKind::SoaColumns { element }
        | LayoutKind::Sequence { element }
        | LayoutKind::Sequential { element } => take(element, refs),
        LayoutKind::Block { payload } | LayoutKind::Box { payload } => take(payload, refs),
        LayoutKind::Callable {
            environment: Some(e),
        } => take(e, refs),
        LayoutKind::Unit
        | LayoutKind::Scalar(_)
        | LayoutKind::Aggregate
        | LayoutKind::Environment { .. }
        | LayoutKind::Variant
        | LayoutKind::Packed
        | LayoutKind::Opaque { .. }
        | LayoutKind::Handle
        | LayoutKind::Path
        | LayoutKind::Arena
        | LayoutKind::Callable { environment: None }
        | LayoutKind::Resource
        | LayoutKind::Header
        | LayoutKind::Flat => {}
    }
    for field in &mut record.fields {
        take(&mut field.layout, refs);
    }
    if let Some(tag) = &mut record.tag {
        for v in &mut tag.variants {
            for field in &mut v.fields {
                take(&mut field.layout, refs);
            }
        }
    }
    scan(&mut record.scan, refs);
    drop(&mut record.drop, refs);
}

snapshot_enum!(OperationArgument { Type(ty), Size(n) });
snapshot_enum!(NativeType { Primitive(p), Tuple(items), Named(path, args), Record(fields), Variant(variants), Function(multiplicity), Box(inner), Builtin(kind, args) });
