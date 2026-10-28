//! SER-1 data schemas, deliberately independent of native storage and modes.
use crate::*;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};

pub const SERIALIZABLE_SCHEMA_VERSION: u32 = 1;

/// Index in one descriptor, never a TypeId or a native layout ID.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub struct SchemaId(pub u32);

/// A type-directed codec description. Declaration and payload order are retained;
/// allocation sharing, loans, brands and native padding have no representation.
/// Parameters describe a checked generic template, not an executable decoder.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SchemaNode {
    Scalar(Primitive),
    Tuple(Vec<SchemaId>),
    Record(Vec<(String, SchemaId)>),
    Variant(Vec<(String, Vec<(Option<String>, SchemaId)>)>),
    Box(SchemaId),
    Builtin {
        kind: BuiltinType,
        arguments: Vec<SchemaArgument>,
    },
    Nominal {
        definition: DefPath,
        representation: SchemaId,
        /// A checked opaque's check function, called on every decoded occurrence
        /// (SER-1). Descriptor bytes do not carry it.
        check: Option<DefPath>,
    },
    Parameter(TypeParameterRef),
    /// Transparent alias or regular recursive application of an earlier schema.
    Reference(SchemaId),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SchemaArgument {
    Type(SchemaId),
    Size(Size),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SerializableSchema {
    pub version: u32,
    pub root: SchemaId,
    pub nodes: Vec<SchemaNode>,
}

impl SerializableSchema {
    /// Canonicalize a graph whose root and child IDs index existing nodes.
    /// Returns None for reference-only cycles. This does not prove eligibility.
    pub fn canonicalized(self) -> Option<Self> {
        canonical(self)
    }
}

/// Prove the complete schema and describe it, or return None for ineligible data.
/// The lookup must supply checked declaration snapshots. Opt-in is necessary but
/// never sufficient: even a nominal's written bit is rechecked against its shape.
/// Local data views may be encoded while valid; modes do not promise reconstructed
/// ownership. Nonregular expansion exhaustion remains an operational QueryError.
pub fn serializable_schema(
    source: &TypeInterner,
    root: TypeId,
    declarations: &GenericDeclarations,
    lookup: &mut impl FnMut(DefId) -> Result<Option<SignatureRef>, QueryError>,
) -> Result<Option<SerializableSchema>, QueryError> {
    schema_inner(source, root, declarations, lookup).map(|schema| schema.and_then(canonical))
}

fn schema_inner(
    source: &TypeInterner,
    root: TypeId,
    declarations: &GenericDeclarations,
    lookup: &mut impl FnMut(DefId) -> Result<Option<SignatureRef>, QueryError>,
) -> Result<Option<SerializableSchema>, QueryError> {
    let store = source;
    let mut schema = SerializableSchema {
        version: SERIALIZABLE_SCHEMA_VERSION,
        root: SchemaId(0),
        nodes: vec![],
    };
    let mut memo = std::collections::HashMap::new();
    let mut pending = vec![];
    fn child(
        id: TypeId,
        schema: &mut SerializableSchema,
        memo: &mut std::collections::HashMap<TypeId, SchemaId>,
        pending: &mut Vec<(TypeId, bool)>,
    ) -> SchemaId {
        *memo.entry(id).or_insert_with(|| {
            let node = SchemaId(schema.nodes.len() as u32);
            // No incomplete descriptor escapes this function.
            schema.nodes.push(SchemaNode::Tuple(vec![]));
            pending.push((id, false));
            node
        })
    }
    child(root, &mut schema, &mut memo, &mut pending);
    let mut expansions = NominalExpansions::default();
    // Every nominal instantiation entered so far, on the current path (recursion) or
    // another one (a type reached twice); a later equal one references its node.
    let mut entered = vec![];
    while let Some((id, exit)) = pending.pop() {
        if exit {
            expansions.leave(id, 0);
            continue;
        }
        let Some(row) = store.get(id).cloned() else {
            return Ok(None);
        };
        if !row.brand.alternatives.is_empty() || row.erased_captures.is_some() {
            return Ok(None);
        }
        let mut part = |id| child(id, &mut schema, &mut memo, &mut pending);
        let node = match row.kind {
            TypeKind::Error | TypeKind::Function(_) | TypeKind::Primitive(Primitive::Never) => {
                return Ok(None);
            }
            TypeKind::Primitive(p) => SchemaNode::Scalar(p),
            TypeKind::Parameter(p) => {
                if !declarations.bounds(p).contains(&Bound::Serializable) {
                    return Ok(None);
                }
                SchemaNode::Parameter(p)
            }
            TypeKind::Tuple(ts) => SchemaNode::Tuple(ts.iter().map(|t| part(*t)).collect()),
            TypeKind::Record(fs) => SchemaNode::Record(
                fs.iter()
                    .map(|f| (f.name.as_str().to_owned(), part(f.ty)))
                    .collect(),
            ),
            TypeKind::Variant(vs) => SchemaNode::Variant(
                vs.iter()
                    .map(|v| {
                        (
                            v.name.as_str().to_owned(),
                            v.fields
                                .iter()
                                .map(|(name, t)| (name.map(|n| n.as_str().to_owned()), part(*t)))
                                .collect(),
                        )
                    })
                    .collect(),
            ),
            TypeKind::Box(t) => SchemaNode::Box(part(t)),
            TypeKind::Builtin { kind, arguments } => {
                let shape: &[bool] = match kind {
                    BuiltinType::Array
                    | BuiltinType::List
                    | BuiltinType::Set
                    | BuiltinType::Packed => &[false],
                    BuiltinType::Map => &[false, false],
                    BuiltinType::Inline | BuiltinType::Simd => &[false, true],
                    _ => return Ok(None),
                };
                if shape.len() != arguments.len()
                    || !shape
                        .iter()
                        .zip(&arguments)
                        .all(|(size, a)| *size == matches!(a, GenericArgument::Size(_)))
                {
                    return Ok(None);
                }
                SchemaNode::Builtin {
                    kind,
                    arguments: arguments
                        .iter()
                        .map(|a| match a {
                            GenericArgument::Type(t) => SchemaArgument::Type(part(*t)),
                            GenericArgument::Size(n) => SchemaArgument::Size(n.clone()),
                        })
                        .collect(),
                }
            }
            TypeKind::Named {
                definition,
                arguments,
            } => {
                let Some(signature) = lookup(definition)? else {
                    return Ok(None);
                };
                let SignatureKind::Type(d) = &signature.kind else {
                    return Ok(None);
                };
                // A core Path is data, not authority: it is transparent text, its lexical
                // spelling, and decoding admits exactly what `path.of` admits.
                if definition.is_core("Path") {
                    if !d.serializable || !arguments.is_empty() {
                        return Ok(None);
                    }
                    let text = SchemaId(schema.nodes.len() as u32);
                    schema.nodes.push(SchemaNode::Scalar(Primitive::String));
                    schema.nodes[memo[&id].0 as usize] = SchemaNode::Nominal {
                        definition: definition.path().clone(),
                        representation: text,
                        check: None,
                    };
                    continue;
                }
                // A generic type declared serializable (core Option and Result, std
                // ReadError[E], ...) need not bound its parameters, so its ordinary
                // declaration still accepts File, resources, etc. Eligibility is proved
                // after substitution.
                let conditional = d.serializable
                    && matches!(&d.authority, AuthoritySummary::Known(terms)
                        if terms.iter().all(|t| matches!(t, AuthorityTerm::TypeParameter(_))));
                let data_authority = d.authority == AuthoritySummary::inert() || conditional;
                // An opaque type is serializable only through its check, over the
                // nongeneric, inert representation it publishes for it.
                let check = d.serializable_check.as_ref();
                if d.capability.is_some()
                    || d.resource
                    || d.drop.is_some()
                    || d.fact.is_some()
                    || (signature.metadata.visibility == hirsel_syntax::Visibility::PublicOpaque)
                        != check.is_some()
                    || check.is_some()
                        && (!d.generics.is_empty() || d.authority != AuthoritySummary::inert())
                    || !data_authority
                    || (!signature.metadata.alias && !d.serializable)
                    || d.generics.len() != arguments.len()
                    || !d.generics.iter().zip(&arguments).all(|(g, a)| {
                        matches!(
                            (g, a),
                            (GenericParameter::Type { .. }, GenericArgument::Type(_))
                                | (GenericParameter::Size { .. }, GenericArgument::Size(_))
                        )
                    })
                {
                    return Ok(None);
                }
                let Some(root) = check.map(|c| c.representation).or(d.representation) else {
                    return Ok(None);
                };
                let check = check.map(|c| c.function.clone());
                if let Some(previous) = entered
                    .iter()
                    .copied()
                    .find(|previous| store.semantic_eq(*previous, id))
                {
                    SchemaNode::Reference(memo[&previous])
                } else {
                    let fresh = expansions.enter(id, 0, store)?;
                    debug_assert!(fresh, "an entered nominal is referenced above");
                    entered.push(id);
                    pending.push((id, true));
                    let Ok(root) =
                        crate::graph::expand(store, root, &arguments, &signature.metadata.roles)
                    else {
                        return Ok(None);
                    };
                    let root = child(root, &mut schema, &mut memo, &mut pending);
                    if signature.metadata.alias {
                        SchemaNode::Reference(root)
                    } else {
                        SchemaNode::Nominal {
                            definition: definition.path().clone(),
                            representation: root,
                            check,
                        }
                    }
                }
            }
        };
        schema.nodes[memo[&id].0 as usize] = node;
    }
    Ok(Some(schema))
}

pub fn structural_serializable(
    source: &TypeInterner,
    root: TypeId,
    declarations: &GenericDeclarations,
    lookup: &mut impl FnMut(DefId) -> Result<Option<SignatureRef>, QueryError>,
) -> Result<bool, QueryError> {
    schema_inner(source, root, declarations, lookup).map(|schema| schema.is_some())
}

// Hashing is only an in-process partition accelerator; it never defines bytes.
impl Hash for SchemaNode {
    fn hash<H: Hasher>(&self, state: &mut H) {
        std::mem::discriminant(self).hash(state);
        match self {
            Self::Scalar(p) => p.hash(state),
            Self::Tuple(ts) => ts.hash(state),
            Self::Record(fs) => fs.hash(state),
            Self::Variant(vs) => vs.hash(state),
            Self::Box(t) | Self::Reference(t) => t.hash(state),
            Self::Builtin { kind, arguments } => {
                std::mem::discriminant(kind).hash(state);
                arguments.hash(state)
            }
            Self::Nominal {
                definition,
                representation,
                check,
            } => {
                definition.hash(state);
                representation.hash(state);
                check.hash(state)
            }
            Self::Parameter(p) => p.hash(state),
        }
    }
}

impl Hash for SchemaArgument {
    fn hash<H: Hasher>(&self, state: &mut H) {
        match self {
            Self::Type(id) => {
                0u8.hash(state);
                id.hash(state);
            }
            Self::Size(Size::Constant(n)) => {
                1u8.hash(state);
                n.hash(state);
            }
            Self::Size(Size::Parameter(p)) => {
                2u8.hash(state);
                p.hash(state);
            }
        }
    }
}

fn parts(node: &SchemaNode, mut map: impl FnMut(SchemaId) -> SchemaId) -> SchemaNode {
    match node {
        SchemaNode::Scalar(p) => SchemaNode::Scalar(*p),
        SchemaNode::Parameter(p) => SchemaNode::Parameter(*p),
        SchemaNode::Tuple(ts) => SchemaNode::Tuple(ts.iter().map(|t| map(*t)).collect()),
        SchemaNode::Record(fs) => {
            SchemaNode::Record(fs.iter().map(|(n, t)| (n.clone(), map(*t))).collect())
        }
        SchemaNode::Variant(vs) => SchemaNode::Variant(
            vs.iter()
                .map(|(n, fs)| {
                    (
                        n.clone(),
                        fs.iter().map(|(n, t)| (n.clone(), map(*t))).collect(),
                    )
                })
                .collect(),
        ),
        SchemaNode::Box(t) => SchemaNode::Box(map(*t)),
        SchemaNode::Reference(t) => SchemaNode::Reference(map(*t)),
        SchemaNode::Nominal {
            definition,
            representation,
            check,
        } => SchemaNode::Nominal {
            definition: definition.clone(),
            representation: map(*representation),
            check: check.clone(),
        },
        SchemaNode::Builtin { kind, arguments } => SchemaNode::Builtin {
            kind: *kind,
            arguments: arguments
                .iter()
                .map(|a| match a {
                    SchemaArgument::Type(t) => SchemaArgument::Type(map(*t)),
                    SchemaArgument::Size(n) => SchemaArgument::Size(n.clone()),
                })
                .collect(),
        },
    }
}

// Finite graph bisimulation removes arena sharing and qualifier-copy artifacts.
// Hash maps are queried in node order and never iterated; randomized placement
// cannot affect the partition or the final rooted declaration-order numbering.
fn canonical(mut schema: SerializableSchema) -> Option<SerializableSchema> {
    let resolve = |mut id: SchemaId| {
        let mut seen = std::collections::BTreeSet::new();
        while let SchemaNode::Reference(next) = &schema.nodes[id.0 as usize] {
            if !seen.insert(id.0) {
                return None;
            }
            id = *next;
        }
        Some(id)
    };
    let targets = (0..schema.nodes.len())
        .map(|i| resolve(SchemaId(i as u32)))
        .collect::<Option<Vec<_>>>()?;
    let nodes: Vec<_> = targets
        .iter()
        .map(|t| parts(&schema.nodes[t.0 as usize], |id| targets[id.0 as usize]))
        .collect();
    let mut classes = vec![SchemaId(0); nodes.len()];
    let mut count = 0;
    loop {
        let mut keys = HashMap::new();
        let next: Vec<_> = nodes
            .iter()
            .map(|node| {
                let key = parts(node, |id| classes[id.0 as usize]);
                let next = SchemaId(keys.len() as u32);
                *keys.entry(key).or_insert(next)
            })
            .collect();
        let stable = keys.len() == count;
        count = keys.len();
        classes = next;
        if stable {
            break;
        }
    }
    let mut representatives = vec![0; count];
    for (index, class) in classes.iter().enumerate() {
        representatives[class.0 as usize] = index;
    }
    let root = classes[schema.root.0 as usize];
    let mut numbering = HashMap::from([(root, SchemaId(0))]);
    let mut queue = vec![root];
    let mut result = vec![];
    let mut cursor = 0;
    while cursor < queue.len() {
        let node = &nodes[representatives[queue[cursor].0 as usize]];
        result.push(parts(node, |id| {
            let class = classes[id.0 as usize];
            *numbering.entry(class).or_insert_with(|| {
                let id = SchemaId(queue.len() as u32);
                queue.push(class);
                id
            })
        }));
        cursor += 1;
    }
    schema.root = SchemaId(0);
    schema.nodes = result;
    Some(schema)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    fn row(kind: TypeKind) -> Type {
        Type {
            kind,
            ..Type::unit()
        }
    }
    fn path(name: &str) -> DefPath {
        DefPath(hirsel_syntax::ItemPath {
            package: hirsel_syntax::PackageId {
                name: "schemas".into(),
                version: "1".into(),
            },
            modules: vec![],
            name: name.into(),
        })
    }
    fn nominal(types: &TypeInterner, kind: TypeKind) -> Signature {
        let root = types.intern(row(kind));
        Signature {
            definition: DefId::intern(&path("Row")),
            metadata: SignatureMetadata::default(),
            fingerprint: Default::default(),
            kind: SignatureKind::Type(Box::new(TypeDefinition {
                loop_source: None,
                format: None,
                generics: vec![],
                representation: Some(root),
                bounds: vec![Bound::Serializable],
                authority: AuthoritySummary::inert(),
                capability: None,
                implementation_authority: None,
                serializable: true,
                serializable_check: None,
                layout: Layout::default(),
                distinct: false,
                resource: false,
                drop: None,
                fact: None,
                captures: None,
                destructor_allocation: Allocation::MayAllocate,
            })),
        }
    }
    fn schema(
        store: &TypeInterner,
        root: TypeId,
        declaration: Option<Signature>,
    ) -> Option<SerializableSchema> {
        serializable_schema(store, root, &GenericDeclarations::default(), &mut |_| {
            Ok(declaration.clone().map(Arc::new))
        })
        .unwrap()
    }
    #[test]
    fn a_nominal_reached_along_two_paths_is_admitted_once() {
        let s = TypeInterner::new(None);
        let named = || {
            row(TypeKind::Named {
                definition: DefId::intern(&path("Row")),
                arguments: vec![].into(),
            })
        };
        let (a, b) = (s.intern(named()), s.intern(named()));
        let pair = s.intern(row(TypeKind::Tuple(vec![a, b].into())));
        let declaration = nominal(&s, TypeKind::Primitive(Primitive::U64));
        let shared = schema(&s, pair, Some(declaration.clone())).expect("admitted");
        let single = s.intern(row(TypeKind::Tuple(vec![a, a].into())));
        assert_eq!(shared, schema(&s, single, Some(declaration)).unwrap());
    }
    #[test]
    fn descriptors_have_schema_ids_and_declaration_order_without_native_layout() {
        let s = TypeInterner::new(None);
        s.intern(row(TypeKind::Primitive(Primitive::Never))); // unreachable rows are irrelevant
        let n = s.intern(row(TypeKind::Primitive(Primitive::U64)));
        let text = s.intern(row(TypeKind::Primitive(Primitive::String)));
        let r = s.intern(row(TypeKind::Record(
            vec![
                Field {
                    name: "second".into(),
                    ty: n,
                    growable: false,
                },
                Field {
                    name: "first".into(),
                    ty: text,
                    growable: false,
                },
            ]
            .into(),
        )));
        let descriptor = schema(&s, r, None).unwrap();
        assert_eq!(descriptor.version, 1);
        assert_eq!(
            descriptor.nodes[0],
            SchemaNode::Record(vec![
                ("second".into(), SchemaId(1)),
                ("first".into(), SchemaId(2))
            ])
        );
        assert_eq!(descriptor.nodes.len(), 3);
        let mut t = s.get(r).unwrap().clone();
        t.modes.relation = Relation::environment();
        t.modes.sendability = Sendability::NotSendable;
        let local = s.intern(t);
        assert_eq!(schema(&s, local, None), Some(descriptor));
    }
    #[test]
    fn opt_in_never_overrides_opaque_resource_authority_or_missing_schema() {
        let s = TypeInterner::new(None);
        let r = s.intern(row(TypeKind::Named {
            definition: DefId::intern(&path("Row")),
            arguments: vec![].into(),
        }));
        let good = nominal(&s, TypeKind::Primitive(Primitive::U64));
        assert!(schema(&s, r, Some(good.clone())).is_some());
        for change in 0..6 {
            let mut bad = good.clone();
            let SignatureKind::Type(d) = &mut bad.kind else {
                unreachable!()
            };
            match change {
                0 => d.serializable = false,
                1 => bad.metadata.visibility = hirsel_syntax::Visibility::PublicOpaque,
                2 => d.resource = true,
                3 => d.authority = AuthoritySummary::Unknown,
                4 => d.representation = None,
                _ => d.capability = Some(CapabilityKind::Global),
            }
            assert!(schema(&s, r, Some(bad)).is_none(), "change {change}");
        }
    }
    #[test]
    fn parameter_evidence_uses_the_exact_binder_and_never_implies_crossing() {
        let p = TypeParameterRef::bound(TypeParamId(0));
        let s = TypeInterner::new(None);
        let mut parameter = row(TypeKind::Parameter(p));
        parameter.modes.sendability = Sendability::DependsOn(vec![p].into());
        let root = s.intern(parameter);
        let g = GenericParameter::Type {
            name: "T".into(),
            bounds: vec![Bound::Serializable].into(),
        };
        let context = GenericDeclarations::signature(&[g]);
        assert!(
            serializable_schema(&s, root, &context, &mut |_| Ok(None))
                .unwrap()
                .is_some()
        );
        assert!(
            serializable_schema(&s, root, &context.nested(&[]), &mut |_| Ok(None))
                .unwrap()
                .is_none()
        );
        assert!(
            !structural_property(
                &s,
                root,
                StructuralProperty::Sendable,
                &context,
                &mut |_| Ok(None)
            )
            .unwrap()
        );
    }
    #[test]
    fn every_unsupported_builtin_and_nested_brand_fails_closed() {
        let s = TypeInterner::new(None);
        let n = s.intern(row(TypeKind::Primitive(Primitive::U64)));
        for kind in [
            BuiltinType::Shared,
            BuiltinType::Slots,
            BuiltinType::SlotId,
            BuiltinType::Mutex,
            BuiltinType::Iter,
            BuiltinType::SortedMap,
            BuiltinType::Ptr,
            BuiltinType::FnPtr,
            BuiltinType::TypeShape,
            BuiltinType::Task,
            BuiltinType::Sender,
            BuiltinType::Receiver,
            BuiltinType::Atomic,
            BuiltinType::Region,
        ] {
            let r = s.intern(row(TypeKind::Builtin {
                kind,
                arguments: vec![GenericArgument::Type(n)].into(),
            }));
            assert!(schema(&s, r, None).is_none(), "{kind:?}");
        }
        let mut branded = s.get(n).unwrap().clone();
        branded.brand.alternatives = vec![Origin {
            root: OriginRoot::Sibling(FieldId(0)),
            fields: Default::default(),
        }]
        .into();
        let branded = s.intern(branded);
        let root = s.intern(row(TypeKind::Box(branded)));
        assert!(schema(&s, root, None).is_none());
        let root = s.intern(row(TypeKind::Box(n)));
        assert!(schema(&s, root, None).is_some());
    }
}

#[cfg(test)]
mod canonical_tests {
    use super::*;
    #[test]
    fn canonical_schemas_ignore_arena_sharing_and_qualifier_copies() {
        let a = TypeInterner::new(None);
        let n = a.intern(Type {
            kind: TypeKind::Primitive(Primitive::U64),
            ..Type::unit()
        });
        let pair = a.intern(Type {
            kind: TypeKind::Tuple(vec![n, n].into()),
            ..Type::unit()
        });
        let b = std::sync::Arc::clone(&a);
        b.intern(Type::boolean());
        let first = b.intern(Type {
            kind: TypeKind::Primitive(Primitive::U64),
            ..Type::unit()
        });
        let mut second = b.get(first).unwrap().clone();
        second.modes.sendability = Sendability::NotSendable;
        let second = b.intern(second);
        let pair_b = b.intern(Type {
            kind: TypeKind::Tuple(vec![first, second].into()),
            ..Type::unit()
        });
        let empty = GenericDeclarations::default();
        let sa = serializable_schema(&a, pair, &empty, &mut |_| Ok(None)).unwrap();
        for _ in 0..8 {
            let sb = serializable_schema(&b, pair_b, &empty, &mut |_| Ok(None)).unwrap();
            assert_eq!(sa, sb);
        }
        assert_eq!(sa.unwrap().nodes.len(), 2);
    }
}
