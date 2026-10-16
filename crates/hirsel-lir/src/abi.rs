//! Hirsel cross-unit ABI v1 derivation (Target and ABI).
//!
//! `FnAbi` is computed once from the specialized declared callable type. Its
//! passing classes read only the declared parameter convention and the
//! public structure of each parameter type: a nominal is always passed by
//! address, so a private representation change moves layout keys but never
//! the ABI, and defining and public views agree. A relation, brand,
//! uniqueness or sendability mode never reaches the key.
use crate::abi_types::*;
use crate::layout::{LayoutBuilder, LayoutError, OwnerRepresentation};
use crate::layout_types::{LayoutId, ScalarKind, TargetSpec};
use crate::snapshot;
use hirsel_core_contract::Passing;
use hirsel_syntax::ContentHash;
use hirsel_types::{BuiltinType, Multiplicity, Primitive, TypeId, TypeKind, TypeStore};

/// How a parameter of one public type travels when not `mut`.
enum Class {
    /// Passed by value in these slots; empty for unit.
    Value(Vec<ScalarKind>),
    /// An inline aggregate or nominal: passed by address.
    Aggregate,
    /// A flat nominal: a borrowed extent is a pointer/length pair.
    Flat,
}

impl<'o> LayoutBuilder<'o> {
    /// The ABI of a declared callable type (a `Function` row of `store`) used
    /// as `shape`. Memoized by the callable's public identity and shape.
    pub fn fn_abi(
        &mut self,
        store: &TypeStore,
        callable: TypeId,
        shape: CallableShape,
    ) -> Result<FnAbiId, LayoutError> {
        if store.id(callable.index()) != Some(callable) {
            return Err(LayoutError::MissingType);
        }
        let row = store.get(callable).ok_or(LayoutError::MissingType)?;
        let function = match &row.kind {
            TypeKind::Function(function) => function,
            TypeKind::Parameter(_) => return Err(LayoutError::Incomplete),
            _ => return Err(LayoutError::Invalid("an ABI of a non-callable type".into())),
        };
        if shape == CallableShape::CWrapper || function.foreign.is_some() {
            return Err(LayoutError::Unavailable {
                issue: Some(6),
                what: "C-facing wrappers and foreign callables".into(),
            });
        }
        if !function.generics.is_empty() {
            return Err(LayoutError::Incomplete);
        }
        let multiplicity = match row.modes.multiplicity {
            Multiplicity::Many => CallMultiplicity::Many,
            Multiplicity::Once => CallMultiplicity::Once,
        };
        let mut key = Vec::new();
        atom(&mut key, b"hirsel.fn-abi.v1");
        atom(&mut key, shape_name(shape).as_bytes());
        atom(&mut key, &[multiplicity as u8]);
        atom(&mut key, &(function.parameters.len() as u64).to_le_bytes());
        for parameter in function.parameters.iter() {
            atom(&mut key, passing_name(parameter.passing.into()).as_bytes());
            self.public_identity(store, parameter.ty, &mut key)?;
        }
        self.public_identity(store, function.result, &mut key)?;
        let result_layout = self.layout(store, function.result)?;
        let result_record = self.record(result_layout);
        let has_views = result_record
            .fields
            .iter()
            .any(|f| crate::layout::field_is_borrowed(self.table(), result_record, f));
        let borrowed_parameters: Vec<_> = if has_views {
            store
                .get(function.result)
                .ok_or(LayoutError::MissingType)?
                .modes
                .relation
                .parameter_roots()
                .iter()
                .map(|p| p.parameter().0)
                .collect()
        } else {
            Vec::new()
        };
        for p in &borrowed_parameters {
            atom(&mut key, &p.to_le_bytes());
        }
        let callable_key = ContentHash::of_parts("hirsel.fn-abi-key.v1", [key.as_slice()]);
        if let Some(&id) = self.abi_memo.get(&(callable_key, shape)) {
            return Ok(id);
        }

        let pointer = PhysicalSlot {
            ty: PhysicalType::Pointer,
            size: self.target().scalar_size(ScalarKind::Pointer),
            align: self.target().scalar_align(ScalarKind::Pointer),
        };
        let mut parameters = Vec::new();
        for (ordinal, parameter) in function.parameters.iter().enumerate() {
            let convention = Passing::from(parameter.passing);
            let layout = self.layout(store, parameter.ty)?;
            // An arena token is one record-address word.
            let class = if self.record(layout).kind == crate::layout_types::LayoutKind::Arena {
                Class::Value(vec![ScalarKind::Pointer])
            } else {
                self.class(store, parameter.ty)?
            };
            let (passing, slots, extension) = match (convention, class) {
                (Passing::Mut, _) => (
                    ParameterPassing::ExclusiveAddress,
                    vec![pointer],
                    ScalarExtension::None,
                ),
                (_, Class::Value(scalars)) => {
                    let extension = match scalars.as_slice() {
                        [s] => extension_of(*s),
                        _ => ScalarExtension::None,
                    };
                    let slots = scalars.iter().map(|&s| slot(self.target(), s)).collect();
                    (ParameterPassing::ByValue, slots, extension)
                }
                (Passing::Borrow, Class::Flat) => (
                    ParameterPassing::PointerLength,
                    vec![pointer, slot(self.target(), ScalarKind::U64)],
                    ScalarExtension::None,
                ),
                (Passing::Borrow, Class::Aggregate) => (
                    ParameterPassing::ReadOnlyAddress,
                    vec![pointer],
                    ScalarExtension::None,
                ),
                (Passing::Take, Class::Aggregate | Class::Flat) => (
                    ParameterPassing::TransferAddress,
                    vec![pointer],
                    ScalarExtension::None,
                ),
            };
            parameters.push(AbiParameter {
                source: ordinal as u32,
                convention,
                passing,
                layout,
                slots,
                extension,
            });
        }
        let result = if self.is_unit(store, function.result)? {
            None
        } else {
            Some(AbiResult {
                borrowed_parameters,
                layout: self.layout(store, function.result)?,
                slots: vec![pointer],
            })
        };
        let destination = result.is_some() && self.may_hold_blocks(store, function.result, 0)?;
        let abi = FnAbi {
            key: FnAbiKey {
                callable: callable_key,
                abi_version: crate::ABI_VERSION,
            },
            shape,
            multiplicity,
            context: true,
            environment: matches!(
                shape,
                CallableShape::Closure | CallableShape::IterNext | CallableShape::IterDrop
            ),
            destination,
            result,
            result_arena: None,
            diverges: matches!(
                store.get(function.result).map(|row| &row.kind),
                Some(TypeKind::Primitive(Primitive::Never))
            ),
            parameters,
            version: crate::ABI_VERSION,
        };
        let id = FnAbiId(self.table_mut_fn_abis().len() as u32);
        self.table_mut_fn_abis().push(abi);
        self.abi_memo.insert((callable_key, shape), id);
        Ok(id)
    }

    /// A direct instance's ABI with explicit ordinary capture parameters.
    /// The supplied layouts have already been derived by this builder.
    pub fn with_capture_parameters(
        &mut self,
        base: FnAbiId,
        captures: &[(LayoutId, Passing)],
        borrows_captures: bool,
    ) -> Result<FnAbiId, LayoutError> {
        let mut abi = self
            .table()
            .fn_abis
            .get(base.0 as usize)
            .ok_or(LayoutError::MissingType)?
            .clone();
        abi.shape = CallableShape::Definition;
        abi.environment = false;
        let borrowed_result = borrows_captures
            && abi.result.as_ref().is_some_and(|result| {
                let record = self.record(result.layout);
                record
                    .fields
                    .iter()
                    .any(|field| crate::layout::field_is_borrowed(self.table(), record, field))
            });
        let pointer = PhysicalSlot {
            ty: PhysicalType::Pointer,
            size: self.target().scalar_size(ScalarKind::Pointer),
            align: self.target().scalar_align(ScalarKind::Pointer),
        };
        let mut key = Vec::new();
        atom(&mut key, &abi.key.callable.0);
        for &(layout, convention) in captures {
            let source = abi.parameters.len() as u32;
            let (passing, slots, extension) = match (convention, &self.record(layout).kind) {
                (Passing::Mut, _) => (
                    ParameterPassing::ExclusiveAddress,
                    vec![pointer],
                    ScalarExtension::None,
                ),
                (_, crate::layout_types::LayoutKind::Scalar(kind)) => (
                    ParameterPassing::ByValue,
                    vec![slot(self.target(), *kind)],
                    extension_of(*kind),
                ),
                (_, crate::layout_types::LayoutKind::Unit) => {
                    (ParameterPassing::ByValue, Vec::new(), ScalarExtension::None)
                }
                (Passing::Borrow, _) => (
                    ParameterPassing::ReadOnlyAddress,
                    vec![pointer],
                    ScalarExtension::None,
                ),
                (Passing::Take, _) => (
                    ParameterPassing::TransferAddress,
                    vec![pointer],
                    ScalarExtension::None,
                ),
            };
            abi.parameters.push(AbiParameter {
                source,
                convention,
                passing,
                layout,
                slots,
                extension,
            });
            if borrowed_result
                && convention == Passing::Borrow
                && let Some(result) = &mut abi.result
            {
                result.borrowed_parameters.push(source);
            }
            atom(
                &mut key,
                hirsel_codec::to_text(&self.record(layout).key).as_bytes(),
            );
            atom(&mut key, passing_name(convention).as_bytes());
        }
        atom(&mut key, &[u8::from(borrowed_result)]);
        abi.key.callable = ContentHash::of_parts("hirsel.capture-abi.v1", [key.as_slice()]);
        if let Some(&id) = self
            .abi_memo
            .get(&(abi.key.callable, CallableShape::Definition))
        {
            return Ok(id);
        }
        let id = FnAbiId(self.table().fn_abis.len() as u32);
        self.abi_memo
            .insert((abi.key.callable, CallableShape::Definition), id);
        self.table_mut_fn_abis().push(abi);
        Ok(id)
    }

    /// The passing class of a parameter type, from its public structure only.
    fn class(&self, store: &TypeStore, ty: TypeId) -> Result<Class, LayoutError> {
        let row = store.get(ty).ok_or(LayoutError::MissingType)?;
        Ok(match &row.kind {
            TypeKind::Primitive(Primitive::String | Primitive::Bytes) => {
                Class::Value(vec![ScalarKind::Pointer, ScalarKind::U64, ScalarKind::U64])
            }
            TypeKind::Primitive(Primitive::Never) => Class::Value(Vec::new()),
            TypeKind::Primitive(p) => Class::Value(vec![scalar_of(*p)]),
            TypeKind::Tuple(items) if items.is_empty() => Class::Value(Vec::new()),
            TypeKind::Function(_) => Class::Value(vec![ScalarKind::Pointer, ScalarKind::Pointer]),
            TypeKind::Box(_) => Class::Value(vec![ScalarKind::Pointer]),
            TypeKind::Builtin { kind, .. } => match kind {
                BuiltinType::Array
                | BuiltinType::List
                | BuiltinType::Map
                | BuiltinType::Set
                | BuiltinType::SortedMap
                | BuiltinType::Slots => Class::Value(vec![ScalarKind::Pointer]),
                BuiltinType::Iter => Class::Value(vec![ScalarKind::Pointer; 3]),
                _ => Class::Aggregate,
            },
            TypeKind::Named { definition, .. }
                if definition.0.package.name == "core"
                    && definition.0.package.version == "1"
                    && definition.0.modules.is_empty()
                    && matches!(definition.0.name.as_str(), "ScalarView" | "PositionView") =>
            {
                Class::Value(vec![ScalarKind::Pointer, ScalarKind::U64, ScalarKind::U64])
            }
            TypeKind::Named { definition, .. } => match self.owners().nominals.get(definition) {
                Some(OwnerRepresentation::Defined(record)) if record.layout_promises.flat => {
                    Class::Flat
                }
                _ => Class::Aggregate,
            },
            TypeKind::Parameter(_) => return Err(LayoutError::Incomplete),
            TypeKind::Error => return Err(LayoutError::Invalid("recovery type in an ABI".into())),
            TypeKind::Tuple(_) | TypeKind::Record(_) | TypeKind::Variant(_) => Class::Aggregate,
        })
    }

    fn is_unit(&self, store: &TypeStore, ty: TypeId) -> Result<bool, LayoutError> {
        let row = store.get(ty).ok_or(LayoutError::MissingType)?;
        Ok(match &row.kind {
            TypeKind::Tuple(items) => items.is_empty(),
            TypeKind::Primitive(Primitive::Never) => true,
            _ => false,
        })
    }

    /// Whether a result of this public type may hold a block, so the caller
    /// supplies an allocation destination. Nominals are opaque: they may.
    fn may_hold_blocks(
        &self,
        store: &TypeStore,
        ty: TypeId,
        depth: u32,
    ) -> Result<bool, LayoutError> {
        if depth > crate::layout::MAX_DEPTH {
            return Err(LayoutError::InstanceLimit);
        }
        let row = store.get(ty).ok_or(LayoutError::MissingType)?;
        let any = |items: &mut dyn Iterator<Item = TypeId>| -> Result<bool, LayoutError> {
            for item in items {
                if self.may_hold_blocks(store, item, depth + 1)? {
                    return Ok(true);
                }
            }
            Ok(false)
        };
        Ok(match &row.kind {
            TypeKind::Primitive(Primitive::String | Primitive::Bytes) => true,
            TypeKind::Primitive(_) => false,
            TypeKind::Tuple(items) => any(&mut items.iter().copied())?,
            TypeKind::Record(fields) => any(&mut fields.iter().map(|f| f.ty))?,
            TypeKind::Variant(variants) => any(&mut variants
                .iter()
                .flat_map(|v| v.fields.iter().map(|(_, t)| *t)))?,
            TypeKind::Builtin {
                kind: BuiltinType::Simd,
                ..
            } => false,
            TypeKind::Builtin {
                kind: BuiltinType::Inline,
                arguments,
            } => match arguments.first() {
                Some(hirsel_types::GenericArgument::Type(element)) => {
                    self.may_hold_blocks(store, *element, depth + 1)?
                }
                _ => true,
            },
            TypeKind::Parameter(_) => return Err(LayoutError::Incomplete),
            _ => true,
        })
    }
}

/// The physical ABI digest: everything callers and callees must agree on,
/// without table-local layout indices. Equal digests mean interchangeable
/// entries.
pub fn abi_digest(abi: &FnAbi) -> ContentHash {
    let mut physical = abi.clone();
    // Body placement proofs do not change the callable machine convention.
    physical.result_arena = None;
    for parameter in &mut physical.parameters {
        parameter.layout = LayoutId(0);
    }
    if let Some(result) = &mut physical.result {
        result.layout = LayoutId(0);
    }
    snapshot::digest("fn-abi-physical", &physical)
}

fn atom(out: &mut Vec<u8>, bytes: &[u8]) {
    out.extend_from_slice(&(bytes.len() as u64).to_le_bytes());
    out.extend_from_slice(bytes);
}

fn shape_name(shape: CallableShape) -> &'static str {
    match shape {
        CallableShape::Definition => "definition",
        CallableShape::Closure => "closure",
        CallableShape::IterNext => "iter-next",
        CallableShape::IterDrop => "iter-drop",
        CallableShape::CWrapper => "c-wrapper",
    }
}

fn passing_name(passing: Passing) -> &'static str {
    match passing {
        Passing::Borrow => "borrow",
        Passing::Take => "take",
        Passing::Mut => "mut",
    }
}

fn scalar_of(p: Primitive) -> ScalarKind {
    match p {
        Primitive::Bool => ScalarKind::Bool,
        Primitive::U8 => ScalarKind::U8,
        Primitive::U16 => ScalarKind::U16,
        Primitive::U32 => ScalarKind::U32,
        Primitive::U64 => ScalarKind::U64,
        Primitive::U128 => ScalarKind::U128,
        Primitive::I8 => ScalarKind::I8,
        Primitive::I16 => ScalarKind::I16,
        Primitive::I32 => ScalarKind::I32,
        Primitive::I64 => ScalarKind::I64,
        Primitive::I128 => ScalarKind::I128,
        Primitive::F32 => ScalarKind::F32,
        Primitive::F64 => ScalarKind::F64,
        Primitive::String | Primitive::Bytes | Primitive::Never => {
            unreachable!("classified before")
        }
    }
}

/// The physical slot of a scalar on `target`.
pub fn slot(target: &TargetSpec, scalar: ScalarKind) -> PhysicalSlot {
    PhysicalSlot {
        ty: physical(scalar),
        size: target.scalar_size(scalar),
        align: target.scalar_align(scalar),
    }
}

pub fn physical(scalar: ScalarKind) -> PhysicalType {
    match scalar {
        ScalarKind::Bool | ScalarKind::U8 | ScalarKind::I8 => PhysicalType::I8,
        ScalarKind::U16 | ScalarKind::I16 => PhysicalType::I16,
        ScalarKind::U32 | ScalarKind::I32 => PhysicalType::I32,
        ScalarKind::U64 | ScalarKind::I64 => PhysicalType::I64,
        ScalarKind::U128 | ScalarKind::I128 => PhysicalType::I128,
        ScalarKind::F32 => PhysicalType::F32,
        ScalarKind::F64 => PhysicalType::F64,
        ScalarKind::Pointer => PhysicalType::Pointer,
    }
}

/// Narrow integers are extended to a full register by the caller.
pub fn extension_of(scalar: ScalarKind) -> ScalarExtension {
    match scalar {
        ScalarKind::Bool | ScalarKind::U8 | ScalarKind::U16 => ScalarExtension::Zero,
        ScalarKind::I8 | ScalarKind::I16 => ScalarExtension::Sign,
        _ => ScalarExtension::None,
    }
}

/// The link name a `RuntimePrimitive` symbol template names for one call
/// (LIR schema revision 6). A template names a monomorphic family by
/// placeholders: `{I}` is the kind of the first scalar argument and `{J}` the
/// result's, both integers; `{F}` is the float kind of the first scalar
/// argument, else of the result (an array reduction takes an array). A name is
/// spelled as its scalar kind (`u8` .. `i128`, `f32`, `f64`). None when a
/// placeholder has no kind of its domain, is unknown, or a brace is
/// unbalanced: such a row has no native symbol for this call.
pub fn resolve_symbol(
    template: &str,
    arguments: &[ScalarKind],
    result: Option<ScalarKind>,
) -> Option<String> {
    let integer = |k: Option<ScalarKind>| k.filter(|k| scalar_name(*k).is_some() && !float(*k));
    let floating = |k: Option<ScalarKind>| k.filter(|k| float(*k));
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(open) = rest.find(['{', '}']) {
        out.push_str(&rest[..open]);
        if !rest[open..].starts_with('{') {
            return None;
        }
        let close = rest[open..].find('}')? + open;
        let kind = match &rest[open + 1..close] {
            "I" => integer(arguments.first().copied())?,
            "J" => integer(result)?,
            "F" => floating(arguments.first().copied()).or(floating(result))?,
            _ => return None,
        };
        out.push_str(scalar_name(kind)?);
        rest = &rest[close + 1..];
    }
    out.push_str(rest);
    Some(out)
}

/// `resolve_symbol` for a call site of `unit`: the scalar kinds of its
/// value arguments in parameter order and of its result.
pub fn resolve_call_symbol<S>(
    template: &str,
    unit: &crate::ir::LirUnit<S>,
    site: &crate::ir::CallSite,
) -> Option<String> {
    use crate::ir::{ArgumentValue, ValueId, ValueRepr};
    let scalar = |v: ValueId| match unit.types[unit.values[v.index()].ty.index()].repr {
        ValueRepr::Scalar(k) => Some(k),
        _ => None,
    };
    let mut arguments = site
        .arguments
        .get(&unit.pools.call_arguments)
        .unwrap_or(&[])
        .to_vec();
    arguments.sort_by_key(|a| a.parameter);
    let kinds: Vec<ScalarKind> = arguments
        .iter()
        .filter_map(|a| match a.value {
            ArgumentValue::Value(v) => scalar(v),
            ArgumentValue::Address { .. } => None,
        })
        .collect();
    resolve_symbol(template, &kinds, site.result.and_then(scalar))
}

fn float(kind: ScalarKind) -> bool {
    matches!(kind, ScalarKind::F32 | ScalarKind::F64)
}

fn scalar_name(kind: ScalarKind) -> Option<&'static str> {
    Some(match kind {
        ScalarKind::U8 => "u8",
        ScalarKind::U16 => "u16",
        ScalarKind::U32 => "u32",
        ScalarKind::U64 => "u64",
        ScalarKind::U128 => "u128",
        ScalarKind::I8 => "i8",
        ScalarKind::I16 => "i16",
        ScalarKind::I32 => "i32",
        ScalarKind::I64 => "i64",
        ScalarKind::I128 => "i128",
        ScalarKind::F32 => "f32",
        ScalarKind::F64 => "f64",
        ScalarKind::Bool | ScalarKind::Pointer => return None,
    })
}
