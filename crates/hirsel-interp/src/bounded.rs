//! Bounded arenas (MEM-6): each open bounded arena's remaining budget, the logical
//! charge of a block placed there, and the boundary that turns the callback's own
//! `OutOfSpace` into `Error(OutOfSpace)`.
//!
//! A charge is taken before the block exists, from the destination type and the
//! constructor's element count, so a failing charge consumes no operand. A deep copy
//! charges every block of the copied value's tree at its length, independently of
//! sharing, which the reference does not observe.
use crate::aggregate::RuntimeTypes;
use crate::value::ArenaStorage;
use crate::{Error, Value, invalid};
use hirsel_syntax::DefId;
use hirsel_tir::Constructor;
use hirsel_type_analysis::logical::{
    Logical, LogicalError, LogicalNominal, Resolved, Scope, type_arguments,
};
use hirsel_types::{BuiltinType, Primitive, TypeId, TypeKind};
use std::collections::BTreeMap;

/// The open bounded arenas, by arena handle id, and the run's own budgets.
pub(crate) struct Budgets {
    issued: u64,
    open: BTreeMap<u64, Budget>,
    pub run: crate::budget::Meter,
    pub storage: crate::sealed_storage::Storage,
}

struct Budget {
    remaining: u64,
    /// The failure record of this arena's own `OutOfSpace`.
    exhausted: Option<usize>,
}

/// The frame of an `arena.bounded` callback: its arena and the number of failure
/// records before the callback began.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Boundary {
    pub arena: u64,
    pub failures: usize,
}

impl Budgets {
    pub fn new(run: crate::RunBudgets) -> Self {
        Self {
            issued: 0,
            storage: Default::default(),
            open: BTreeMap::new(),
            run: crate::budget::Meter::new(run),
        }
    }
    /// Every scope, bounded or unbounded, receives a fresh semantic identity.
    pub fn issue(&mut self) -> u64 {
        self.issued += 1;
        self.issued
    }
    pub fn open(&mut self, arena: u64, cap: u64) {
        self.open.insert(
            arena,
            Budget {
                remaining: cap,
                exhausted: None,
            },
        );
    }
    pub fn bounded(&self, arena: u64) -> bool {
        self.open.contains_key(&arena)
    }
    /// Take `bytes` from the arena's budget, or report that they exceed it.
    pub fn charge(&mut self, arena: u64, bytes: u64) -> Result<(), u64> {
        let Some(budget) = self.open.get_mut(&arena) else {
            return Ok(());
        };
        match budget.remaining.checked_sub(bytes) {
            Some(remaining) => {
                budget.remaining = remaining;
                Ok(())
            }
            None => Err(arena),
        }
    }
    /// The arena's `OutOfSpace` is failure record `index`.
    pub fn exhausted(&mut self, arena: u64, index: usize) {
        if let Some(budget) = self.open.get_mut(&arena) {
            budget.exhausted = Some(index);
        }
    }
    /// Close the boundary's arena. True when its callback failed with exactly the
    /// arena's own `OutOfSpace`: the first record after the callback began, with no
    /// failure during its cleanup.
    pub fn close(&mut self, boundary: Boundary, failures: usize) -> bool {
        let budget = self.open.remove(&boundary.arena);
        budget.and_then(|b| b.exhausted) == Some(boundary.failures)
            && failures == boundary.failures + 1
    }
}

fn resolver(runtime: &RuntimeTypes) -> impl Fn(DefId) -> Option<LogicalNominal> + '_ {
    move |definition: DefId| {
        runtime
            .get(definition.path())
            .map(|n| match n.representation {
                Some(shape) => LogicalNominal::Representation { shape },
                None => LogicalNominal::Opaque,
            })
    }
}

fn logical_error(error: LogicalError) -> Error {
    match error {
        LogicalError::Parameter => {
            Error::Unavailable("a bounded-arena charge of a generic type".into())
        }
        LogicalError::Unresolved(path) => Error::Unavailable(format!(
            "a bounded-arena charge of {:?} without its representation",
            path.0.name
        )),
        LogicalError::Malformed => invalid("a malformed bounded-arena charge type"),
    }
}

/// The charge of constructing `count` fields into a block of type `ty`: a box its
/// payload, a List or Array its elements, a Map or Set its entries; inline values
/// occupy no arena block.
pub(crate) fn construction<'a>(
    runtime: &'a RuntimeTypes,
    ty: TypeId,
    constructor: &Constructor,
    count: usize,
) -> Result<u64, Error> {
    let resolve = resolver(runtime);
    let logical = Logical::new(&resolve);
    let scope = Scope::root(&runtime.types);
    let charge = || -> Result<u64, LogicalError> {
        let Resolved::Structural(row, scope) = logical.resolve(ty, &scope)? else {
            return Ok(0);
        };
        Ok(match (constructor, &row.kind) {
            (Constructor::Box, TypeKind::Box(payload)) => logical.size(*payload, &scope)?,
            (Constructor::Sequence | Constructor::Map | Constructor::Set, _) => {
                match logical.element(ty, &Scope::root(&runtime.types))? {
                    Some(element) => element.saturating_mul(count as u64),
                    None => 0,
                }
            }
            _ => 0,
        })
    };
    charge().map_err(logical_error)
}

/// The logical bytes of one element of the List or Array type `ty`, or one
/// entry of the Map or Set type `ty`: what a push or an insert of a new key
/// charges, and per element what a copy-on-write copy charges (MEM-6).
pub(crate) fn element<'a>(runtime: &'a RuntimeTypes, ty: TypeId) -> Result<u64, Error> {
    let resolve = resolver(runtime);
    let logical = Logical::new(&resolve);
    let element = logical
        .element(ty, &Scope::root(&runtime.types))
        .map_err(logical_error)?;
    Ok(element.unwrap_or(0))
}

/// The arena storage of a block of `kind` built in `arena` (MEM-6): a List,
/// Array, Map or Set, `None` for any other. Its per-element charge is read from
/// `ty` only when the arena is bounded: an unbounded arena is never charged.
pub(crate) fn storage<'a>(
    runtime: &'a RuntimeTypes,
    ty: TypeId,
    kind: BuiltinType,
    arena: u64,
    bounded: bool,
) -> Result<Option<ArenaStorage>, Error> {
    if !matches!(
        kind,
        BuiltinType::List
            | BuiltinType::Array
            | BuiltinType::Map
            | BuiltinType::Set
            | BuiltinType::SortedMap
    ) {
        return Ok(None);
    }
    let element = if bounded { element(runtime, ty)? } else { 0 };
    Ok(Some(ArenaStorage::fresh(arena, element)))
}

/// Retag every List, Array, Map and Set block of `value`'s tree as fresh
/// storage of `arena`, or every one as heap storage when it is None: a deep
/// copy (`copy_to`) builds every block anew, unshared. Element charges are
/// read from `ty` only for a bounded arena.
pub(crate) fn retag<'a>(
    runtime: &'a RuntimeTypes,
    ty: TypeId,
    value: &mut Value,
    arena: Option<(u64, bool)>,
) -> Result<(), Error> {
    let Some((arena, bounded)) = arena else {
        let mut work = vec![value];
        while let Some(value) = work.pop() {
            value.retag_own_block(None);
            if let Some(fields) = value.fields_mut() {
                work.extend(fields.iter_mut().flatten());
            }
        }
        return Ok(());
    };
    if !bounded {
        let mut work = vec![value];
        while let Some(value) = work.pop() {
            value.retag_own_block(Some(ArenaStorage::fresh(arena, 0)));
            if let Some(fields) = value.fields_mut() {
                work.extend(fields.iter_mut().flatten());
            }
        }
        return Ok(());
    }
    let resolve = resolver(runtime);
    let logical = Logical::new(&resolve);
    let mut work = vec![(value, ty, Scope::root(&runtime.types))];
    while let Some((value, ty, scope)) = work.pop() {
        let Resolved::Structural(row, scope) =
            logical.resolve(ty, &scope).map_err(logical_error)?
        else {
            continue;
        };
        let mismatch = || invalid("a copied value that does not match its type");
        let value = if matches!(
            &row.kind,
            TypeKind::Builtin {
                kind: BuiltinType::SortedMap,
                ..
            }
        ) {
            match value {
                Value::Record { fields, .. } => fields
                    .get_mut(1)
                    .and_then(Option::as_mut)
                    .ok_or_else(mismatch)?,
                _ => return Err(mismatch()),
            }
        } else {
            value
        };
        if matches!(
            value,
            Value::Bytes(_)
                | Value::ArenaBytes { .. }
                | Value::String(_)
                | Value::ArenaString { .. }
                | Value::Box(_)
                | Value::ArenaBox { .. }
                | Value::Closure { .. }
        ) {
            value.retag_own_block(Some(ArenaStorage::fresh(arena, 0)));
        }
        match (&row.kind, value) {
            (TypeKind::Box(payload), Value::Box(fields) | Value::ArenaBox { fields, .. }) => {
                for field in fields.iter_mut().flatten() {
                    work.push((field, *payload, scope.clone()));
                }
            }
            (TypeKind::Builtin { kind, arguments }, value) => {
                let types = type_arguments(arguments);
                match (kind, types.as_slice(), value) {
                    (
                        BuiltinType::List | BuiltinType::Array,
                        [element, ..],
                        Value::Sequence {
                            elements,
                            arena: tag,
                            ..
                        },
                    ) => {
                        let each = logical.size(*element, &scope).map_err(logical_error)?;
                        *tag = Some(ArenaStorage::fresh(arena, each));
                        for element_value in elements.iter_mut().flatten() {
                            work.push((element_value, *element, scope.clone()));
                        }
                    }
                    (
                        BuiltinType::Set,
                        [element, ..],
                        Value::Set {
                            entries,
                            arena: tag,
                        },
                    ) => {
                        let each = logical.size(*element, &scope).map_err(logical_error)?;
                        *tag = Some(ArenaStorage::fresh(arena, each));
                        for entry in entries.iter_mut().flatten() {
                            work.push((entry, *element, scope.clone()));
                        }
                    }
                    (
                        BuiltinType::Map | BuiltinType::SortedMap,
                        [key, item, ..],
                        Value::Map {
                            entries,
                            arena: tag,
                        }
                        | Value::Sequence {
                            elements: entries,
                            arena: tag,
                            ..
                        },
                    ) => {
                        let each = logical
                            .size(*key, &scope)
                            .and_then(|k| Ok(k.saturating_add(logical.size(*item, &scope)?)))
                            .map_err(logical_error)?;
                        *tag = Some(ArenaStorage::fresh(arena, each));
                        for entry in entries.iter_mut().flatten() {
                            let Value::Tuple(parts) = entry else {
                                return Err(mismatch());
                            };
                            if let [Some(k), Some(v)] = parts.as_mut_slice() {
                                work.push((k, *key, scope.clone()));
                                work.push((v, *item, scope.clone()));
                            }
                        }
                    }
                    (
                        BuiltinType::Inline | BuiltinType::Simd,
                        [element, ..],
                        Value::Sequence { elements, .. },
                    ) => {
                        for element_value in elements.iter_mut().flatten() {
                            work.push((element_value, *element, scope.clone()));
                        }
                    }
                    (BuiltinType::Packed, [element, ..], value) => {
                        work.push((value, *element, scope.clone()));
                    }
                    _ => {}
                }
            }
            (
                TypeKind::Function(_),
                Value::Closure {
                    captures,
                    environment,
                    ..
                },
            ) => {
                if captures.len() != environment.capture_types.len()
                    || captures.len() != environment.owned_captures.len()
                {
                    return Err(invalid("unclosed copied environment"));
                }
                for ((value, ty), owned) in captures
                    .iter_mut()
                    .zip(&environment.capture_types)
                    .zip(&environment.owned_captures)
                {
                    if *owned && let Some(value) = value {
                        work.push((value, *ty, Scope::root(&runtime.types)));
                    }
                }
            }
            (TypeKind::Tuple(items), Value::Tuple(fields)) => {
                for (item, field) in items.iter().zip(fields.iter_mut()) {
                    if let Some(field) = field {
                        work.push((field, *item, scope.clone()));
                    }
                }
            }
            (
                TypeKind::Record(declared),
                Value::Record { fields, .. } | Value::Distinct { fields, .. },
            ) => {
                for (field, value) in declared.iter().zip(fields.iter_mut()) {
                    if let Some(value) = value {
                        work.push((value, field.ty, scope.clone()));
                    }
                }
            }
            (TypeKind::Variant(variants), Value::Variant { variant, fields }) => {
                let declared = variants.get(*variant as usize).ok_or_else(mismatch)?;
                for ((_, ty), value) in declared.fields.iter().zip(fields.iter_mut()) {
                    if let Some(value) = value {
                        work.push((value, *ty, scope.clone()));
                    }
                }
            }
            _ => {}
        }
    }
    Ok(())
}

/// The charge of a closure environment of these capture types.
pub(crate) fn environment<'a>(
    runtime: &'a RuntimeTypes,
    captures: &[TypeId],
) -> Result<u64, Error> {
    let resolve = resolver(runtime);
    let logical = Logical::new(&resolve);
    let scope = Scope::root(&runtime.types);
    let mut total = 0u64;
    for capture in captures {
        total = total.saturating_add(logical.size(*capture, &scope).map_err(logical_error)?);
    }
    Ok(total)
}

/// The charge of deep-copying `value` of type `ty`: every block of its tree.
pub(crate) fn copy<'a>(
    runtime: &'a RuntimeTypes,
    ty: TypeId,
    value: &'a Value,
) -> Result<u64, Error> {
    let resolve = resolver(runtime);
    let logical = Logical::new(&resolve);
    let mut work = vec![(value, ty, Scope::root(&runtime.types))];
    let mut total = 0u64;
    while let Some((value, ty, scope)) = work.pop() {
        let Resolved::Structural(row, scope) =
            logical.resolve(ty, &scope).map_err(logical_error)?
        else {
            continue;
        };
        let size = |ty: TypeId| logical.size(ty, &scope).map_err(logical_error);
        let mismatch = || invalid("a copied value that does not match its type");
        let value = if matches!(
            &row.kind,
            TypeKind::Builtin {
                kind: BuiltinType::SortedMap,
                ..
            }
        ) {
            match value {
                Value::Record { fields, .. } => fields
                    .get(1)
                    .and_then(Option::as_ref)
                    .ok_or_else(mismatch)?,
                _ => return Err(mismatch()),
            }
        } else {
            value
        };
        match (&row.kind, value) {
            (
                TypeKind::Primitive(Primitive::String),
                Value::String(text) | Value::ArenaString { text, .. },
            ) => {
                total = total.saturating_add(text.len() as u64);
            }
            (
                TypeKind::Primitive(Primitive::Bytes),
                Value::Bytes(bytes) | Value::ArenaBytes { bytes, .. },
            ) => {
                total = total.saturating_add(bytes.len() as u64);
            }
            (TypeKind::Primitive(_), _) => {}
            (TypeKind::Box(payload), Value::Box(fields) | Value::ArenaBox { fields, .. }) => {
                total = total.saturating_add(size(*payload)?);
                for field in fields.iter().flatten() {
                    work.push((field, *payload, scope.clone()));
                }
            }
            (TypeKind::Builtin { kind, arguments }, _) => {
                let types = type_arguments(arguments);
                match (kind, types.as_slice(), value) {
                    (
                        BuiltinType::List
                        | BuiltinType::Array
                        | BuiltinType::Inline
                        | BuiltinType::Simd,
                        [element, ..],
                        Value::Sequence { elements, .. },
                    ) => {
                        if matches!(kind, BuiltinType::List | BuiltinType::Array) {
                            let each = size(*element)?;
                            total =
                                total.saturating_add(each.saturating_mul(elements.len() as u64));
                        }
                        for element_value in elements.iter().flatten() {
                            work.push((element_value, *element, scope.clone()));
                        }
                    }
                    (
                        BuiltinType::Set,
                        [element, ..],
                        Value::Set {
                            entries: elements, ..
                        },
                    ) => {
                        let each = size(*element)?;
                        total = total.saturating_add(
                            each.saturating_mul(elements.iter().flatten().count() as u64),
                        );
                        for element_value in elements.iter().flatten() {
                            work.push((element_value, *element, scope.clone()));
                        }
                    }
                    (
                        BuiltinType::Map | BuiltinType::SortedMap,
                        [key, item, ..],
                        Value::Map { entries, .. }
                        | Value::Sequence {
                            elements: entries, ..
                        },
                    ) => {
                        let each = size(*key)?.saturating_add(size(*item)?);
                        total = total.saturating_add(
                            each.saturating_mul(entries.iter().flatten().count() as u64),
                        );
                        for entry in entries.iter().flatten() {
                            let Value::Tuple(parts) = entry else {
                                return Err(mismatch());
                            };
                            if let [Some(k), Some(v)] = parts.as_slice() {
                                work.push((k, *key, scope.clone()));
                                work.push((v, *item, scope.clone()));
                            }
                        }
                    }
                    (BuiltinType::Packed, [element, ..], _) => {
                        work.push((value, *element, scope.clone()));
                    }
                    _ => {
                        return Err(Error::Unavailable(format!(
                            "a bounded-arena copy of a {kind:?}"
                        )));
                    }
                }
            }
            (TypeKind::Tuple(items), Value::Tuple(fields)) => {
                for (item, field) in items.iter().zip(fields) {
                    if let Some(field) = field {
                        work.push((field, *item, scope.clone()));
                    }
                }
            }
            (TypeKind::Tuple(items), Value::Unit) if items.is_empty() => {}
            (
                TypeKind::Record(declared),
                Value::Record { fields, .. }
                | Value::Distinct { fields, .. }
                | Value::Resource { fields, .. },
            ) => {
                for (field, value) in declared.iter().zip(fields) {
                    if let Some(value) = value {
                        work.push((value, field.ty, scope.clone()));
                    }
                }
            }
            (
                TypeKind::Variant(variants),
                Value::Variant { variant, fields }
                | Value::ResourceVariant {
                    variant, fields, ..
                },
            ) => {
                let declared = variants.get(*variant as usize).ok_or_else(mismatch)?;
                for ((_, ty), value) in declared.fields.iter().zip(fields) {
                    if let Some(value) = value {
                        work.push((value, *ty, scope.clone()));
                    }
                }
            }
            (TypeKind::Function(_), Value::FunctionReference(_)) => (),
            (
                TypeKind::Function(_),
                Value::Closure {
                    captures,
                    environment,
                    ..
                },
            ) => {
                if captures.len() != environment.capture_types.len()
                    || captures.len() != environment.owned_captures.len()
                {
                    return Err(invalid("unclosed copied environment"));
                }
                for ((value, ty), owned) in captures
                    .iter()
                    .zip(&environment.capture_types)
                    .zip(&environment.owned_captures)
                {
                    if *owned && let Some(value) = value {
                        total = total.saturating_add(
                            logical
                                .size(*ty, &Scope::root(&runtime.types))
                                .map_err(logical_error)?,
                        );
                        work.push((value, *ty, Scope::root(&runtime.types)));
                    }
                }
            }
            _ => return Err(mismatch()),
        }
    }
    Ok(total)
}

/// The logical element size of an operation type actual. Its rows and checked
/// nominal representations use the runtime's shared type interner.
pub(crate) fn operation_size(call: &crate::OperationCall<'_>, ty: TypeId) -> Result<u64, Error> {
    let resolve = resolver(call.runtime);
    Logical::new(&resolve)
        .size(ty, &Scope::root(call.types))
        .map_err(logical_error)
}
