//! Hand-built semantic type graphs for tests of layouts, ABIs and runtime descriptors.
//! Rows retain fixture defaults; no semantic property is inferred from their kind.
use crate::*;
use hirsel_syntax::PassingMode;

/// Rows under construction, in their own session interner `store`.
pub struct Types {
    pub store: std::sync::Arc<TypeInterner>,
}
impl Default for Types {
    fn default() -> Self {
        Self {
            store: TypeInterner::new(None),
        }
    }
}

impl Types {
    pub fn add_with(&mut self, kind: TypeKind, modes: Modes) -> TypeId {
        self.store.intern(Type {
            modes,
            ..Type::with_kind(kind)
        })
    }
    pub fn add(&mut self, kind: TypeKind) -> TypeId {
        self.add_with(kind, Modes::global())
    }
    pub fn p(&mut self, p: Primitive) -> TypeId {
        self.add(TypeKind::Primitive(p))
    }
    pub fn tuple(&mut self, items: &[TypeId]) -> TypeId {
        self.add(TypeKind::Tuple(items.to_vec().into()))
    }
    pub fn record(&mut self, fields: &[TypeId]) -> TypeId {
        self.add(TypeKind::Record(
            fields
                .iter()
                .enumerate()
                .map(|(i, &ty)| Field {
                    growable: false,
                    name: hirsel_intern::Symbol::intern(&format!("f{i}")),
                    ty,
                })
                .collect(),
        ))
    }
    pub fn growable_record(&mut self, fields: &[TypeId]) -> TypeId {
        let last = fields.len() - 1;
        self.add(TypeKind::Record(
            fields
                .iter()
                .enumerate()
                .map(|(i, &ty)| Field {
                    growable: i == last,
                    name: hirsel_intern::Symbol::intern(&format!("f{i}")),
                    ty,
                })
                .collect(),
        ))
    }
    pub fn variant(&mut self, variants: &[&[TypeId]]) -> TypeId {
        self.add(TypeKind::Variant(
            variants
                .iter()
                .enumerate()
                .map(|(i, fields)| Variant {
                    name: hirsel_intern::Symbol::intern(&format!("V{i}")),
                    fields: fields.iter().map(|&t| (None, t)).collect(),
                })
                .collect(),
        ))
    }
    pub fn named(&mut self, definition: &DefPath, arguments: Vec<GenericArgument>) -> TypeId {
        self.add(TypeKind::Named {
            definition: DefId::intern(definition),
            arguments: arguments.into(),
        })
    }
    pub fn builtin(&mut self, kind: BuiltinType, arguments: Vec<GenericArgument>) -> TypeId {
        self.add(TypeKind::Builtin {
            kind,
            arguments: arguments.into(),
        })
    }
    pub fn list(&mut self, element: TypeId) -> TypeId {
        self.builtin(BuiltinType::List, vec![GenericArgument::Type(element)])
    }
    pub fn inline(&mut self, element: TypeId, n: u64) -> TypeId {
        self.builtin(
            BuiltinType::Inline,
            vec![
                GenericArgument::Type(element),
                GenericArgument::Size(Size::Constant(n)),
            ],
        )
    }
    pub fn boxed(&mut self, inner: TypeId) -> TypeId {
        self.add(TypeKind::Box(inner))
    }
    pub fn param(&mut self, i: u32) -> TypeId {
        self.add(TypeKind::Parameter(BinderRef::bound(TypeParamId(i))))
    }
    pub fn function(
        &mut self,
        parameters: &[(PassingMode, TypeId)],
        result: TypeId,
        m: Multiplicity,
        generics: u32,
    ) -> TypeId {
        self.add_with(
            TypeKind::Function(FunctionType {
                state_promises: Default::default(),
                generics: (0..generics)
                    .map(|i| GenericParameter::Type {
                        name: format!("T{i}"),
                        bounds: Vec::new().into(),
                    })
                    .collect(),
                parameters: parameters
                    .iter()
                    .map(|&(passing, ty)| Parameter {
                        name: None,
                        label: None,
                        passing,
                        ty,
                    })
                    .collect(),
                result,
                error: None,
                allocation: Allocation::MayAllocate,
                authority: AuthoritySummary::inert(),
                uses: Uses::Unrestricted,
                contracts: Vec::new().into(),
                incomplete: false,
                foreign: None,
            }),
            Modes {
                multiplicity: m,
                ..Modes::global()
            },
        )
    }

    pub fn unit(&mut self) -> TypeId {
        self.add(TypeKind::Tuple(Vec::new().into()))
    }

    pub fn simd(&mut self, element: TypeId, lanes: u64) -> TypeId {
        self.add(TypeKind::Builtin {
            kind: BuiltinType::Simd,
            arguments: vec![
                GenericArgument::Type(element),
                GenericArgument::Size(Size::Constant(lanes)),
            ]
            .into(),
        })
    }
}
