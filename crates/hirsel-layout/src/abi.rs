//! Hirsel cross-unit ABI v5 derivation (Target and ABI).
//!
//! `FnAbi` is computed once from the specialized declared callable type. Its
//! passing classes read only the declared parameter convention and the
//! public structure of each parameter type: a nominal is always passed by
//! address, so a private representation change moves layout keys but never
//! the ABI, and defining and public views agree. A relation, brand,
//! uniqueness or sendability mode never reaches the key.
use crate::layout::{LayoutBuilder, OwnerRepresentation};
use hirsel_core_contract::Passing;
use hirsel_lir::abi_types::*;
use hirsel_lir::layout::LayoutError;
use hirsel_lir::layout_types::{LayoutId, ScalarKind};
use hirsel_syntax::ContentHash;
use hirsel_types::{BuiltinType, Multiplicity, Primitive, TypeId, TypeInterner, TypeKind};

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
    fn ordinary_abi(&self, id: FnAbiId) -> Result<&FnAbi, LayoutError> {
        let abi = self
            .table()
            .fn_abis
            .get(id.0 as usize)
            .ok_or(LayoutError::MissingType)?;
        if abi.receipts.is_some() || abi.key.receipt_contract.is_some() {
            return Err(LayoutError::Invalid(
                "receipt-aware ABI in ordinary request namespace".into(),
            ));
        }
        Ok(abi)
    }

    fn ordinary_cached(
        &self,
        callable: ContentHash,
        shape: CallableShape,
    ) -> Result<Option<FnAbiId>, LayoutError> {
        let Some(&id) = self.abi_memo.get(&(callable, shape)) else {
            return Ok(None);
        };
        self.ordinary_abi(id)?;
        Ok(Some(id))
    }

    /// Specialize a completed ordinary ABI in a separate content-keyed request
    /// namespace. Apply site, capture and scoped decorations before this step.
    /// The record remains a claim until C compares current source requirements.
    pub fn receipt_fn_abi(
        &mut self,
        base: FnAbiId,
        contract: &hirsel_lir::sealed_payload_source::ReceiptContract,
    ) -> Result<FnAbiId, LayoutError> {
        contract
            .check_structure()
            .map_err(|e| LayoutError::Invalid(e.into()))?;
        let mut abi = self.ordinary_abi(base)?.clone();
        if abi.shape == CallableShape::Foreign {
            return Err(LayoutError::Invalid(
                "receipt carriers require a Hirsel adapter, never a foreign ABI".into(),
            ));
        }
        let content = contract
            .content()
            .map_err(|e| LayoutError::Invalid(e.into()))?;
        if let Some(&id) = self.receipt_abi_memo.get(&(base, content)) {
            return Ok(id);
        }
        abi.key.receipt_contract = Some(content);
        abi.receipts = Some(contract.clone());
        hirsel_lir::abi::check_receipt_contract(&abi, self.table())
            .map_err(|e| LayoutError::Invalid(e.into()))?;
        let id = FnAbiId(self.table().fn_abis.len() as u32);
        self.table_mut_fn_abis().push(abi);
        self.receipt_abi_memo.insert((base, content), id);
        Ok(id)
    }

    /// The ABI of a declared callable type (a `Function` row of `types`) used
    /// as `shape`. Memoized by the callable's public identity and shape.
    pub fn fn_abi(
        &mut self,
        types: &TypeInterner,
        callable: TypeId,
        shape: CallableShape,
    ) -> Result<FnAbiId, LayoutError> {
        let row = types.get(callable).ok_or(LayoutError::MissingType)?;
        let function = match &row.kind {
            TypeKind::Function(function) => function,
            TypeKind::Parameter(_) => return Err(LayoutError::Incomplete),
            _ => return Err(LayoutError::Invalid("an ABI of a non-callable type".into())),
        };
        if (shape == CallableShape::Foreign) != function.foreign.is_some() {
            return Err(LayoutError::Invalid(
                "only an extern has the foreign shape, and an extern has only it".into(),
            ));
        }
        if !function.generics.is_empty() {
            return Err(LayoutError::Incomplete);
        }
        let multiplicity = match row.modes.multiplicity {
            Multiplicity::Many => CallMultiplicity::Many,
            Multiplicity::Once => CallMultiplicity::Once,
        };
        let mut key = Vec::new();
        atom(&mut key, b"hirsel.fn-abi.v2");
        atom(&mut key, shape_name(shape).as_bytes());
        atom(&mut key, &[multiplicity as u8]);
        atom(&mut key, &(function.parameters.len() as u64).to_le_bytes());
        for parameter in function.parameters.iter() {
            atom(&mut key, passing_name(parameter.passing.into()).as_bytes());
            self.public_identity(types, parameter.ty, &mut key)?;
        }
        self.public_identity(types, function.result, &mut key)?;
        let result_layout = self.layout(types, function.result)?;
        let result_record = self.record(result_layout);
        let has_views = result_record
            .fields
            .iter()
            .any(|f| hirsel_lir::layout::field_is_borrowed(self.table(), result_record, f));
        let borrowed_parameters: Vec<_> = if has_views {
            types
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
        // Only declaration-typed callable parameters imply CLO-1. A type
        // parameter that later specializes to a callable grants no new promise.
        let nonretaining: Vec<_> = function
            .parameters
            .iter()
            .enumerate()
            .filter(|(_, p)| {
                p.passing != hirsel_types::PassingMode::Take
                    && matches!(
                        types.get(p.ty).map(|t| &t.kind),
                        Some(TypeKind::Function(_))
                    )
            })
            .map(|(i, _)| ParameterPath {
                parameter: i as u32,
                fields: Vec::new(),
            })
            .collect();
        let mut scoped_entries = Vec::new();
        for (ordinal, parameter) in function.parameters.iter().enumerate() {
            if matches!(types.get(parameter.ty).map(|r| &r.kind), Some(TypeKind::Function(f)) if f.generics.is_empty() && f.foreign.is_none())
            {
                let nested = self.fn_abi(types, parameter.ty, CallableShape::Closure)?;
                scoped_entries.push(ScopedEntry {
                    parameter: ParameterPath {
                        parameter: ordinal as u32,
                        fields: Vec::new(),
                    },
                    identity: hirsel_lir::abi::scoped_entry_identity(
                        &self.table().fn_abis[nested.0 as usize],
                        self.table(),
                    ),
                });
            }
        }
        let scoped_contract = hirsel_lir::abi::scoped_contract_digest(&nonretaining, &[]);
        atom(
            &mut key,
            &hirsel_lir::snapshot::digest("scoped-entries-v1", &scoped_entries).0,
        );
        atom(&mut key, &scoped_contract.0);
        let callable_key = ContentHash::of_parts("hirsel.fn-abi-key.v1", [key.as_slice()]);
        if let Some(id) = self.ordinary_cached(callable_key, shape)? {
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
            let layout = self.layout(types, parameter.ty)?;
            // An arena token is one record-address word.
            let class = if self.record(layout).kind == hirsel_lir::layout_types::LayoutKind::Arena {
                Class::Value(vec![ScalarKind::Pointer])
            } else {
                self.class(types, parameter.ty)?
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
        let result = if self.is_unit(types, function.result)? {
            None
        } else {
            Some(AbiResult {
                borrowed_parameters,
                layout: self.layout(types, function.result)?,
                slots: vec![pointer],
                register: (shape != CallableShape::Foreign)
                    .then(|| hirsel_lir::abi::register_kind(&self.record(result_layout).kind))
                    .flatten(),
            })
        };
        // C returns its admitted scalar/nullable pointer in a register. A
        // nominal Option does not acquire Hirsel's conservative block destination.
        let destination = shape != CallableShape::Foreign
            && result.is_some()
            && self.may_hold_blocks(types, function.result, 0)?;
        let abi = FnAbi {
            receipts: None,
            nonretaining,
            nonretaining_captures: Vec::new(),
            scoped_contract,
            scoped_entries,
            key: FnAbiKey {
                receipt_contract: None,
                callable: callable_key,
                abi_version: hirsel_lir::ABI_VERSION,
            },
            shape,
            multiplicity,
            context: shape != CallableShape::Foreign,
            environment: matches!(
                shape,
                CallableShape::Closure | CallableShape::IterNext | CallableShape::IterDrop
            ),
            caller_site: matches!(
                shape,
                CallableShape::Closure | CallableShape::IterNext | CallableShape::IterDrop
            ),
            destination,
            result,
            result_arena: None,
            diverges: matches!(
                types.get(function.result).map(|row| &row.kind),
                Some(TypeKind::Primitive(Primitive::Never))
            ),
            parameters,
            version: hirsel_lir::ABI_VERSION,
        };
        let id = FnAbiId(self.table_mut_fn_abis().len() as u32);
        self.table_mut_fn_abis().push(abi);
        self.abi_memo.insert((callable_key, shape), id);
        Ok(id)
    }

    /// Add definition-owned capture selectors without changing any machine slots.
    pub fn with_scoped_contract(
        &mut self,
        base: FnAbiId,
        parameters: Vec<ParameterPath>,
        captures: Vec<ParameterPath>,
    ) -> Result<FnAbiId, LayoutError> {
        let mut abi = self.ordinary_abi(base)?.clone();
        if abi.nonretaining == parameters && abi.nonretaining_captures == captures {
            return Ok(base);
        }
        abi.nonretaining = parameters;
        abi.nonretaining_captures = captures;
        abi.scoped_contract =
            hirsel_lir::abi::scoped_contract_digest(&abi.nonretaining, &abi.nonretaining_captures);
        abi.key.callable = ContentHash::of_parts(
            "hirsel.scoped-capture-abi.v1",
            [
                abi.key.callable.0.as_slice(),
                abi.scoped_contract.0.as_slice(),
            ],
        );
        if let Some(id) = self.ordinary_cached(abi.key.callable, abi.shape)? {
            return Ok(id);
        }
        let id = FnAbiId(self.table().fn_abis.len() as u32);
        self.abi_memo.insert((abi.key.callable, abi.shape), id);
        self.table_mut_fn_abis().push(abi);
        Ok(id)
    }

    /// Bind a higher-order actual to a body-proved invocation contract.
    pub fn with_scoped_entry(
        &mut self,
        base: FnAbiId,
        parameter: ParameterPath,
        nested: FnAbiId,
    ) -> Result<FnAbiId, LayoutError> {
        let identity =
            hirsel_lir::abi::scoped_entry_identity(self.ordinary_abi(nested)?, self.table());
        let mut abi = self.ordinary_abi(base)?.clone();
        let entry = ScopedEntry {
            parameter,
            identity,
        };
        if abi.scoped_entries.iter().any(|e| *e == entry) {
            return Ok(base);
        }
        abi.scoped_entries
            .retain(|e| e.parameter != entry.parameter);
        abi.scoped_entries.push(entry);
        abi.scoped_entries
            .sort_by(|a, b| a.parameter.cmp(&b.parameter));
        abi.key.callable = ContentHash::of_parts(
            "hirsel.scoped-entry-abi.v1",
            [
                abi.key.callable.0.as_slice(),
                hirsel_lir::snapshot::digest("scoped-entries-v1", &abi.scoped_entries)
                    .0
                    .as_slice(),
            ],
        );
        if let Some(id) = self.ordinary_cached(abi.key.callable, abi.shape)? {
            return Ok(id);
        }
        let id = FnAbiId(self.table().fn_abis.len() as u32);
        self.abi_memo.insert((abi.key.callable, abi.shape), id);
        self.table_mut_fn_abis().push(abi);
        Ok(id)
    }

    /// Add the constant caller-site carrier without changing the callable's language type.
    pub fn with_caller_site(&mut self, base: FnAbiId) -> Result<FnAbiId, LayoutError> {
        let mut abi = self.ordinary_abi(base)?.clone();
        if abi.caller_site {
            return Ok(base);
        }
        if !abi.context {
            return Err(LayoutError::Invalid(
                "caller metadata on a foreign ABI".into(),
            ));
        }
        abi.caller_site = true;
        abi.key.callable =
            ContentHash::of_parts("hirsel.caller-abi.v1", [abi.key.callable.0.as_slice()]);
        if let Some(id) = self.ordinary_cached(abi.key.callable, abi.shape)? {
            return Ok(id);
        }
        let id = FnAbiId(self.table().fn_abis.len() as u32);
        self.abi_memo.insert((abi.key.callable, abi.shape), id);
        self.table_mut_fn_abis().push(abi);
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
        let mut abi = self.ordinary_abi(base)?.clone();
        let parameter_count = abi.parameters.len() as u32;
        for capture in std::mem::take(&mut abi.nonretaining_captures) {
            abi.nonretaining.push(ParameterPath {
                parameter: parameter_count + capture.parameter,
                fields: capture.fields,
            });
        }
        abi.nonretaining.sort();
        abi.scoped_contract = hirsel_lir::abi::scoped_contract_digest(&abi.nonretaining, &[]);
        abi.shape = CallableShape::Definition;
        abi.environment = false;
        let borrowed_result = borrows_captures
            && abi.result.as_ref().is_some_and(|result| {
                let record = self.record(result.layout);
                record
                    .fields
                    .iter()
                    .any(|field| hirsel_lir::layout::field_is_borrowed(self.table(), record, field))
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
                (_, hirsel_lir::layout_types::LayoutKind::Scalar(kind)) => (
                    ParameterPassing::ByValue,
                    vec![slot(self.target(), *kind)],
                    extension_of(*kind),
                ),
                (_, hirsel_lir::layout_types::LayoutKind::Unit) => {
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
        if let Some(id) = self.ordinary_cached(abi.key.callable, CallableShape::Definition)? {
            return Ok(id);
        }
        let id = FnAbiId(self.table().fn_abis.len() as u32);
        self.abi_memo
            .insert((abi.key.callable, CallableShape::Definition), id);
        self.table_mut_fn_abis().push(abi);
        Ok(id)
    }

    /// The passing class of a parameter type, from its public structure only.
    fn class(&self, types: &TypeInterner, ty: TypeId) -> Result<Class, LayoutError> {
        let row = types.get(ty).ok_or(LayoutError::MissingType)?;
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
                | BuiltinType::Slots
                | BuiltinType::Shared => Class::Value(vec![ScalarKind::Pointer]),
                BuiltinType::Iter => Class::Value(vec![ScalarKind::Pointer; 3]),
                BuiltinType::Ptr | BuiltinType::FnPtr => Class::Value(vec![ScalarKind::Pointer]),
                _ => Class::Aggregate,
            },
            TypeKind::Named { definition, .. }
                if definition.path().package_is("core", "1")
                    && definition.path().0.modules.is_empty()
                    && matches!(
                        definition.path().0.name.as_str(),
                        "ScalarView" | "PositionView"
                    ) =>
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

    fn is_unit(&self, types: &TypeInterner, ty: TypeId) -> Result<bool, LayoutError> {
        let row = types.get(ty).ok_or(LayoutError::MissingType)?;
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
        types: &TypeInterner,
        ty: TypeId,
        depth: u32,
    ) -> Result<bool, LayoutError> {
        if depth > hirsel_lir::layout::MAX_DEPTH {
            return Err(LayoutError::InstanceLimit);
        }
        let row = types.get(ty).ok_or(LayoutError::MissingType)?;
        let any = |items: &mut dyn Iterator<Item = TypeId>| -> Result<bool, LayoutError> {
            for item in items {
                if self.may_hold_blocks(types, item, depth + 1)? {
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
                kind: BuiltinType::Simd | BuiltinType::Ptr | BuiltinType::FnPtr,
                ..
            } => false,
            TypeKind::Builtin {
                kind: BuiltinType::Inline,
                arguments,
            } => match arguments.first() {
                Some(hirsel_types::GenericArgument::Type(element)) => {
                    self.may_hold_blocks(types, *element, depth + 1)?
                }
                _ => true,
            },
            TypeKind::Parameter(_) => return Err(LayoutError::Incomplete),
            _ => true,
        })
    }
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
        CallableShape::Foreign => "foreign",
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
        Primitive::U32 | Primitive::Char => ScalarKind::U32,
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

use hirsel_lir::abi::{extension_of, slot};

#[cfg(test)]
mod receipt_tests;
