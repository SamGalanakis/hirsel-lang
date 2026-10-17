//! Logical sizes (MEM-6): the target-independent bytes a bounded arena charges for a
//! value, read from type structure alone, never from a physical layout.
//!
//! A scalar is its width, a reference to a block (`box T`, `String`, `Bytes`, `List`,
//! `Array`, `Map`, `Set`, `SortedMap`) or an opaque nominal (a capability, handle or
//! arena token) is 8, a callable or `Iter` is 16, a tuple, record or resource is the sum
//! of its fields, a variant is 4 plus its largest variant's fields, and `Inline[T, n]`
//! and `Simd[T, n]` are `n` times `T`. Padding, headers and slabs are never charged.
use crate::{
    BinderRef, BuiltinType, GenericArgument, Primitive, Size, Type, TypeId, TypeKind, TypeStore,
};
use hirsel_syntax::DefPath;
use std::rc::Rc;

/// What a nominal type contributes: its representation's shape in that
/// representation's own store, whose `Bound { depth: 0 }` parameters are the
/// nominal's generic actuals, or an opaque word.
#[derive(Clone, Copy, Debug)]
pub enum LogicalNominal<'a> {
    Representation { types: &'a TypeStore, shape: TypeId },
    Opaque,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LogicalError {
    /// A type or size parameter with no actual: a generic body running parametrically.
    Parameter,
    /// A nominal type the resolver does not know.
    Unresolved(DefPath),
    /// A row missing from its store, or a representation nested past any real type.
    Malformed,
}

/// The tag a variant is charged besides its largest variant's fields.
pub const VARIANT_TAG: u64 = 4;
/// A block reference, an opaque nominal or an arena token.
pub const REFERENCE: u64 = 8;
/// A callable (code and environment) or an erased `Iter`.
pub const CALLABLE: u64 = 16;

const MAX_NESTING: usize = 256;

/// The store a type row belongs to and the actuals of its `Bound { depth: 0 }`
/// parameters: the root store has none, a nominal representation's store has
/// the nominal's, which belong to the enclosing scope.
#[derive(Clone)]
pub struct Scope<'a>(Rc<Frame<'a>>);

struct Frame<'a> {
    store: &'a TypeStore,
    actuals: &'a [GenericArgument],
    parent: Option<Scope<'a>>,
}

impl<'a> Scope<'a> {
    pub fn root(store: &'a TypeStore) -> Self {
        Scope(Rc::new(Frame {
            store,
            actuals: &[],
            parent: None,
        }))
    }
    pub fn store(&self) -> &'a TypeStore {
        self.0.store
    }
}

/// A structural row reached through parameters and nominal representations,
/// with the scope its child types belong to.
pub enum Resolved<'a> {
    Structural(&'a Type, Scope<'a>),
    Opaque,
}

/// Logical sizes and resolved structure under one nominal resolver.
pub struct Logical<'a, 'r> {
    nominal: &'r dyn Fn(&DefPath) -> Option<LogicalNominal<'a>>,
}

impl<'a, 'r> Logical<'a, 'r> {
    pub fn new(nominal: &'r dyn Fn(&DefPath) -> Option<LogicalNominal<'a>>) -> Self {
        Self { nominal }
    }

    /// Follow parameters to their actuals and nominals to their representations.
    pub fn resolve(&self, ty: TypeId, scope: &Scope<'a>) -> Result<Resolved<'a>, LogicalError> {
        let (mut ty, mut scope) = (ty, scope.clone());
        for _ in 0..MAX_NESTING {
            let row = scope.store().get(ty).ok_or(LogicalError::Malformed)?;
            match &row.kind {
                TypeKind::Parameter(BinderRef::Bound {
                    depth: 0,
                    parameter,
                }) => {
                    let Some(parent) = scope.0.parent.clone() else {
                        return Err(LogicalError::Parameter);
                    };
                    match scope.0.actuals.get(parameter.0 as usize) {
                        Some(GenericArgument::Type(actual)) => (ty, scope) = (*actual, parent),
                        _ => return Err(LogicalError::Malformed),
                    }
                }
                TypeKind::Parameter(_) => return Err(LogicalError::Parameter),
                TypeKind::Named {
                    definition,
                    arguments,
                } => match (self.nominal)(definition)
                    .ok_or_else(|| LogicalError::Unresolved(definition.clone()))?
                {
                    LogicalNominal::Opaque => return Ok(Resolved::Opaque),
                    LogicalNominal::Representation { types, shape } => {
                        scope = Scope(Rc::new(Frame {
                            store: types,
                            actuals: arguments,
                            parent: Some(scope),
                        }));
                        ty = shape;
                    }
                },
                TypeKind::Error => return Err(LogicalError::Malformed),
                _ => return Ok(Resolved::Structural(row, scope)),
            }
        }
        Err(LogicalError::Malformed)
    }

    /// A generic size argument's value.
    pub fn count(&self, size: &Size, scope: &Scope<'a>) -> Result<u64, LogicalError> {
        let (mut size, mut scope) = (size, scope.clone());
        for _ in 0..MAX_NESTING {
            match size {
                Size::Constant(n) => return Ok(*n),
                Size::Parameter(BinderRef::Bound {
                    depth: 0,
                    parameter,
                }) => {
                    let Some(parent) = scope.0.parent.clone() else {
                        return Err(LogicalError::Parameter);
                    };
                    match scope.0.actuals.get(parameter.0 as usize) {
                        Some(GenericArgument::Size(actual)) => (size, scope) = (actual, parent),
                        _ => return Err(LogicalError::Malformed),
                    }
                }
                Size::Parameter(_) => return Err(LogicalError::Parameter),
            }
        }
        Err(LogicalError::Malformed)
    }

    /// The logical size of a value of `ty`.
    pub fn size(&self, ty: TypeId, scope: &Scope<'a>) -> Result<u64, LogicalError> {
        self.size_at(ty, scope, 0)
    }

    /// The logical element size of a `List`, `Array`, `Map` (key plus value) or
    /// `Set` block type, the unit of its capacity charge. None for any other type.
    pub fn element(&self, ty: TypeId, scope: &Scope<'a>) -> Result<Option<u64>, LogicalError> {
        let Resolved::Structural(row, scope) = self.resolve(ty, scope)? else {
            return Ok(None);
        };
        let TypeKind::Builtin { kind, arguments } = &row.kind else {
            return Ok(None);
        };
        let types = type_arguments(arguments);
        let parts: &[TypeId] = match (kind, types.as_slice()) {
            (BuiltinType::List | BuiltinType::Array | BuiltinType::Set, [element, ..]) => {
                std::slice::from_ref(element)
            }
            (BuiltinType::Map | BuiltinType::SortedMap, [key, value, ..]) => &types[..2],
            _ => return Ok(None),
        };
        let mut total = 0u64;
        for part in parts {
            total = total.saturating_add(self.size(*part, &scope)?);
        }
        Ok(Some(total))
    }

    fn size_at(&self, ty: TypeId, scope: &Scope<'a>, nesting: usize) -> Result<u64, LogicalError> {
        if nesting > MAX_NESTING {
            return Err(LogicalError::Malformed);
        }
        let Resolved::Structural(row, scope) = self.resolve(ty, scope)? else {
            return Ok(REFERENCE);
        };
        let sum = |parts: &mut dyn Iterator<Item = TypeId>| -> Result<u64, LogicalError> {
            let mut total = 0u64;
            for part in parts {
                total = total.saturating_add(self.size_at(part, &scope, nesting + 1)?);
            }
            Ok(total)
        };
        Ok(match &row.kind {
            TypeKind::Primitive(p) => primitive(*p),
            TypeKind::Tuple(items) => sum(&mut items.iter().copied())?,
            TypeKind::Record(fields) => sum(&mut fields.iter().map(|field| field.ty))?,
            TypeKind::Variant(variants) => {
                let mut largest = 0u64;
                for variant in variants.iter() {
                    largest = largest.max(sum(&mut variant.fields.iter().map(|(_, t)| *t))?);
                }
                VARIANT_TAG.saturating_add(largest)
            }
            TypeKind::Function(_) => CALLABLE,
            TypeKind::Box(_) => REFERENCE,
            TypeKind::Builtin { kind, arguments } => {
                let element = type_arguments(arguments).first().copied();
                let count = arguments.iter().find_map(|a| match a {
                    GenericArgument::Size(s) => Some(s),
                    GenericArgument::Type(_) => None,
                });
                match (kind, element, count) {
                    (BuiltinType::Inline | BuiltinType::Simd, Some(element), Some(count)) => self
                        .count(count, &scope)?
                        .saturating_mul(self.size_at(element, &scope, nesting + 1)?),
                    (BuiltinType::Packed, Some(element), _) => {
                        self.size_at(element, &scope, nesting + 1)?
                    }
                    (BuiltinType::Iter, _, _) => CALLABLE,
                    _ => REFERENCE,
                }
            }
            TypeKind::Named { .. } | TypeKind::Parameter(_) | TypeKind::Error => {
                return Err(LogicalError::Malformed);
            }
        })
    }
}

/// The type actuals of a builtin, in order.
pub fn type_arguments(arguments: &[GenericArgument]) -> Vec<TypeId> {
    arguments
        .iter()
        .filter_map(|a| match a {
            GenericArgument::Type(t) => Some(*t),
            GenericArgument::Size(_) => None,
        })
        .collect()
}

/// A primitive's logical width.
pub fn primitive(p: Primitive) -> u64 {
    match p {
        Primitive::Never => 0,
        Primitive::Bool | Primitive::U8 | Primitive::I8 => 1,
        Primitive::U16 | Primitive::I16 => 2,
        Primitive::U32 | Primitive::I32 | Primitive::F32 => 4,
        Primitive::U64 | Primitive::I64 | Primitive::F64 => 8,
        Primitive::U128 | Primitive::I128 => 16,
        Primitive::String | Primitive::Bytes => REFERENCE,
    }
}

/// The logical size of `ty` in `store`.
pub fn logical_size<'a>(
    store: &'a TypeStore,
    ty: TypeId,
    nominal: &dyn Fn(&DefPath) -> Option<LogicalNominal<'a>>,
) -> Result<u64, LogicalError> {
    Logical::new(nominal).size(ty, &Scope::root(store))
}

/// The logical element size of a block type in `store` (see [`Logical::element`]).
pub fn logical_element<'a>(
    store: &'a TypeStore,
    ty: TypeId,
    nominal: &dyn Fn(&DefPath) -> Option<LogicalNominal<'a>>,
) -> Result<Option<u64>, LogicalError> {
    Logical::new(nominal).element(ty, &Scope::root(store))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Field, Type, TypeParamId, Variant};

    fn push(store: &mut TypeStore, kind: TypeKind) -> TypeId {
        store.push(Type::with_kind(kind))
    }

    #[test]
    fn sizes_follow_the_logical_table() {
        let mut store = TypeStore::default();
        let byte = push(&mut store, TypeKind::Primitive(Primitive::U8));
        let word = push(&mut store, TypeKind::Primitive(Primitive::U64));
        let text = push(&mut store, TypeKind::Primitive(Primitive::String));
        let pair = push(&mut store, TypeKind::Tuple(vec![byte, word].into()));
        let boxed = push(&mut store, TypeKind::Box(pair));
        let either = push(
            &mut store,
            TypeKind::Variant(
                vec![
                    Variant {
                        name: "Small".into(),
                        fields: vec![(None, byte)].into(),
                    },
                    Variant {
                        name: "Large".into(),
                        fields: vec![(None, pair), (None, text)].into(),
                    },
                ]
                .into(),
            ),
        );
        let inline = push(
            &mut store,
            TypeKind::Builtin {
                kind: BuiltinType::Inline,
                arguments: vec![GenericArgument::Type(word), GenericArgument::Size(Size::Constant(3))]
                    .into(),
            },
        );
        let list = push(
            &mut store,
            TypeKind::Builtin {
                kind: BuiltinType::List,
                arguments: vec![GenericArgument::Type(pair)].into(),
            },
        );
        let none = |_: &DefPath| None;
        assert_eq!(logical_size(&store, pair, &none), Ok(9));
        assert_eq!(logical_size(&store, boxed, &none), Ok(8));
        assert_eq!(logical_size(&store, either, &none), Ok(4 + 9 + 8));
        assert_eq!(logical_size(&store, inline, &none), Ok(24));
        assert_eq!(logical_size(&store, list, &none), Ok(8));
        assert_eq!(logical_element(&store, list, &none), Ok(Some(9)));
        assert_eq!(logical_element(&store, pair, &none), Ok(None));
    }

    #[test]
    fn a_nominal_charges_its_representation_with_its_actuals() {
        let mut representation = TypeStore::default();
        let parameter = push(
            &mut representation,
            TypeKind::Parameter(BinderRef::Bound {
                depth: 0,
                parameter: TypeParamId(0),
            }),
        );
        let word = push(&mut representation, TypeKind::Primitive(Primitive::U64));
        let shape = push(
            &mut representation,
            TypeKind::Record(
                vec![
                    Field {
                        growable: false,
                        name: "first".into(),
                        ty: parameter,
                    },
                    Field {
                        growable: false,
                        name: "second".into(),
                        ty: word,
                    },
                ]
                .into(),
            ),
        );
        let mut store = TypeStore::default();
        let short = push(&mut store, TypeKind::Primitive(Primitive::U16));
        let path = DefPath::core("Pair");
        let named = push(
            &mut store,
            TypeKind::Named {
                definition: path.clone(),
                arguments: vec![GenericArgument::Type(short)].into(),
            },
        );
        let generic = push(
            &mut store,
            TypeKind::Parameter(BinderRef::Bound {
                depth: 0,
                parameter: TypeParamId(0),
            }),
        );
        let other = push(
            &mut store,
            TypeKind::Named {
                definition: DefPath::core("Other"),
                arguments: vec![].into(),
            },
        );
        let resolve = |p: &DefPath| {
            (*p == path).then_some(LogicalNominal::Representation {
                types: &representation,
                shape,
            })
        };
        assert_eq!(logical_size(&store, named, &resolve), Ok(10));
        assert_eq!(
            logical_size(&store, generic, &resolve),
            Err(LogicalError::Parameter)
        );
        assert_eq!(
            logical_size(&store, other, &resolve),
            Err(LogicalError::Unresolved(DefPath::core("Other")))
        );
    }
}
