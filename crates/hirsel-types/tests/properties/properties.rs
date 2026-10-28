//! `prove` decides Plain, Copyable, NeedsRelease, Sendable and Flat, and
//! `structural_equality` Data and Key, for J, I, V and RC alike, so their
//! agreement campaign cannot see a bug in them. These differentials check
//! them against naive recursive references written from the spec. GEN-1: plain is
//! "copyable and holds no blocks"; copying needs every owned part copyable and
//! is never available for a `once` value (MODE-3); release is needed exactly
//! when a value is not plain (DROP-1/MEM-1). The graphs are random but
//! deterministic, so a failure names a seed that reproduces it.
use super::*;
use std::collections::BTreeSet;

/// The parameters every graph may reference: `T0`, `T1: plain`,
/// `T2: copyable` and `T3: data`.
fn declarations() -> Vec<GenericParameter> {
    [
        vec![],
        vec![Bound::Plain],
        vec![Bound::Copyable],
        vec![Bound::Data],
    ]
    .into_iter()
    .enumerate()
    .map(|(i, bounds)| GenericParameter::Type {
        name: format!("T{i}").as_str().into(),
        bounds: bounds.into(),
    })
    .collect()
}

pub(super) struct Random(pub(super) u64);
impl Random {
    pub(super) fn below(&mut self, n: u64) -> u64 {
        // xorshift64*: deterministic, and enough to spread small graphs.
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d) % n
    }
}

fn row(kind: TypeKind, once: bool) -> Type {
    let mut t = Type::boolean();
    t.kind = kind;
    if once {
        t.modes.multiplicity = Multiplicity::Once;
    }
    t
}

/// A random acyclic graph of at most `depth` levels; every row is global and
/// aliased, so only the structure decides the three properties.
fn generate(r: &mut Random, store: &TypeInterner, depth: u32) -> TypeId {
    let leaf = depth == 0;
    let choice = if leaf { r.below(5) } else { r.below(12) };
    let child = |r: &mut Random, store: &TypeInterner| generate(r, store, depth - 1);
    let kind = match choice {
        0 => TypeKind::Primitive(
            [Primitive::U8, Primitive::U64, Primitive::Bool][r.below(3) as usize],
        ),
        1 => TypeKind::Primitive([Primitive::String, Primitive::Bytes][r.below(2) as usize]),
        2 => TypeKind::Parameter(TypeParameterRef::bound(TypeParamId(r.below(4) as u32))),
        3 => {
            let mut f = Type::boolean();
            f.kind = TypeKind::Tuple(vec![].into());
            let unit = store.intern(f);
            TypeKind::Function(FunctionType {
                state_promises: Default::default(),
                generics: vec![].into(),
                parameters: vec![].into(),
                result: unit,
                error: None,
                allocation: Allocation::NoAlloc,
                authority: AuthoritySummary::inert(),
                uses: Uses::Only(AuthoritySummary::inert()),
                contracts: vec![].into(),
                incomplete: false,
                foreign: None,
            })
        }
        4 => TypeKind::Tuple(vec![].into()),
        5 | 6 => {
            let n = r.below(4);
            TypeKind::Tuple((0..n).map(|_| child(r, store)).collect::<Vec<_>>().into())
        }
        7 => TypeKind::Variant(
            (0..1 + r.below(3))
                .map(|v| Variant {
                    name: format!("V{v}").as_str().into(),
                    fields: (0..r.below(3))
                        .map(|i| (Some(format!("f{i}").as_str().into()), child(r, store)))
                        .collect::<Vec<_>>()
                        .into(),
                })
                .collect::<Vec<_>>()
                .into(),
        ),
        8 => TypeKind::Box(child(r, store)),
        9 => TypeKind::Builtin {
            kind: BuiltinType::List,
            arguments: vec![GenericArgument::Type(child(r, store))].into(),
        },
        10 => TypeKind::Builtin {
            kind: BuiltinType::Inline,
            arguments: vec![
                GenericArgument::Type(child(r, store)),
                GenericArgument::Size(Size::Constant(2)),
            ]
            .into(),
        },
        _ => TypeKind::Builtin {
            kind: BuiltinType::Map,
            arguments: vec![
                GenericArgument::Type(child(r, store)),
                GenericArgument::Type(child(r, store)),
            ]
            .into(),
        },
    };
    let once = matches!(kind, TypeKind::Function(_)) && r.below(2) == 0;
    store.intern(row(kind, once))
}

fn type_arguments(arguments: &[GenericArgument]) -> Vec<TypeId> {
    arguments
        .iter()
        .filter_map(|a| match a {
            GenericArgument::Type(t) => Some(*t),
            _ => None,
        })
        .collect()
}

/// Every owned part: tuple elements, variant payloads, box payloads and
/// container elements. A function value's header owns nothing.
fn parts(t: &Type) -> Vec<TypeId> {
    match &t.kind {
        TypeKind::Tuple(ts) => ts.to_vec(),
        TypeKind::Variant(vs) => vs
            .iter()
            .flat_map(|v| v.fields.iter().map(|f| f.1))
            .collect(),
        TypeKind::Box(t) => vec![*t],
        TypeKind::Builtin { arguments, .. } => type_arguments(arguments),
        _ => vec![],
    }
}

fn bounded(p: &TypeParameterRef, implied: &[Bound]) -> bool {
    let GenericParameter::Type { bounds, .. } = &declarations()[p.parameter().0 as usize] else {
        return false;
    };
    bounds.iter().any(|b| implied.contains(b))
}

fn reference_copyable(store: &TypeInterner, id: TypeId) -> bool {
    let t = store.get(id).unwrap();
    if t.modes.multiplicity == Multiplicity::Once {
        return false;
    }
    match &t.kind {
        // GEN-1: data, plain and copyable all permit copying.
        TypeKind::Parameter(p) => bounded(p, &[Bound::Plain, Bound::Copyable, Bound::Data]),
        _ => parts(t).into_iter().all(|p| reference_copyable(store, p)),
    }
}

fn reference_plain(store: &TypeInterner, id: TypeId) -> bool {
    let t = store.get(id).unwrap();
    // Holding a block: strings, bytes, boxes, lists, maps and callables.
    let holds_block = match &t.kind {
        TypeKind::Primitive(p) => matches!(p, Primitive::String | Primitive::Bytes),
        TypeKind::Box(_) | TypeKind::Function(_) => true,
        TypeKind::Builtin { kind, .. } => *kind != BuiltinType::Inline,
        TypeKind::Parameter(p) => !bounded(p, &[Bound::Plain]),
        _ => false,
    };
    !holds_block
        && reference_copyable(store, id)
        && parts(t).into_iter().all(|p| reference_plain(store, p))
}

#[test]
fn prove_agrees_with_a_naive_reference_on_random_type_graphs() {
    let declarations = declarations();
    let generics = GenericDeclarations::signature(&declarations);
    let mut lookup = |_: DefId| Ok::<_, QueryError>(None);
    let mut counts = [0usize; 2];
    for seed in 1..=4000u64 {
        let mut r = Random(seed.wrapping_mul(0x9e37_79b9_7f4a_7c15));
        let store = TypeInterner::new(None);
        let root = generate(&mut r, &store, 1 + (seed % 4) as u32);
        let plain = reference_plain(&store, root);
        counts[usize::from(plain)] += 1;
        for (property, expected) in [
            (StructuralProperty::Plain, plain),
            (StructuralProperty::NeedsRelease, !plain),
            (
                StructuralProperty::Copyable,
                reference_copyable(&store, root),
            ),
        ] {
            let actual =
                structural_property(&store, root, property, &generics, &mut lookup).unwrap();
            assert_eq!(
                actual,
                expected,
                "seed {seed}, {property:?} of {:?}",
                store.get(root).unwrap().kind
            );
        }
    }
    // Both verdicts occur often enough for the comparison to mean something.
    assert!(counts.iter().all(|n| *n > 500), "{counts:?}");
}

/// Adding a part can only take a property away: a tuple is plain or
/// copyable only if each element is, and needs release if any element does.
#[test]
fn properties_are_monotone_under_field_addition() {
    let declarations = declarations();
    let generics = GenericDeclarations::signature(&declarations);
    let mut lookup = |_: DefId| Ok::<_, QueryError>(None);
    for seed in 1..=2000u64 {
        let mut r = Random(seed.wrapping_mul(0xd1b5_4a32_d192_ed03));
        let store = TypeInterner::new(None);
        let base = generate(&mut r, &store, 2);
        let added = generate(&mut r, &store, 2);
        let before = store.intern(row(TypeKind::Tuple(vec![base].into()), false));
        let after = store.intern(row(TypeKind::Tuple(vec![base, added].into()), false));
        for property in [StructuralProperty::Plain, StructuralProperty::Copyable] {
            let mut proved =
                |t| structural_property(&store, t, property, &generics, &mut lookup).unwrap();
            let (after, before, added) = (proved(after), proved(before), proved(added));
            assert!(!after || before && added, "seed {seed}: {property:?}");
        }
        let mut release = |t| {
            structural_property(
                &store,
                t,
                StructuralProperty::NeedsRelease,
                &generics,
                &mut lookup,
            )
            .unwrap()
        };
        assert!(
            !release(before) || release(after),
            "seed {seed}: NeedsRelease"
        );
    }
}

// Crossing (MODE-4), flatness (MEM-12) and equality (GEN-1). These graphs
// carry value modes, nominal declarations through a stub lookup, handles and
// every bound set, and the references are written from the spec text, not
// from `prove`: J, I and V all read `prove`, so only an independent reading
// catches a rule it drops.

/// The root-scope parameter `Ti` has the bounds `CROSSING_BOUNDS[i]`.
const CROSSING_BOUNDS: [&[Bound]; 10] = [
    &[],
    &[Bound::Plain],
    &[Bound::Copyable],
    &[Bound::Data],
    &[Bound::Sendable],
    &[Bound::Flat],
    &[Bound::Sendable, Bound::Copyable],
    &[Bound::Key],
    &[Bound::Inert],
    &[Bound::Sendable, Bound::Data],
];

fn crossing_declarations() -> Vec<GenericParameter> {
    CROSSING_BOUNDS
        .iter()
        .enumerate()
        .map(|(i, bounds)| GenericParameter::Type {
            name: format!("T{i}").as_str().into(),
            bounds: bounds.to_vec().into(),
        })
        .collect()
}

/// What a nominal declaration exports. An opaque declaration's bounds are
/// trusted, so each set here is one some representation proves; a private
/// declaration exports no bound and is judged by its representation.
#[derive(Clone, Copy, Debug)]
pub(super) enum Declared {
    Transparent,
    Opaque(&'static [Bound]),
    /// A `flat type`: its fields may own bytes, and it is flat, not plain.
    FlatLayout,
    Resource,
    GlobalCapability,
    ScopedCapability,
}
const OPAQUE_BOUNDS: [&[Bound]; 9] = [
    &[],
    &[Bound::Sendable],
    &[Bound::Copyable],
    &[Bound::Plain, Bound::Copyable, Bound::Sendable],
    &[Bound::Flat, Bound::Plain, Bound::Copyable, Bound::Sendable],
    &[Bound::Flat, Bound::Copyable, Bound::Sendable],
    &[Bound::Data, Bound::Copyable],
    &[Bound::Key, Bound::Data, Bound::Copyable],
    &[Bound::Sendable, Bound::Data, Bound::Copyable],
];

pub(super) struct Declaration {
    pub(super) declared: Declared,
    pub(super) generics: usize,
    /// The representation, which an opaque declaration's signature hides as
    /// a client sees it.
    pub(super) representation: TypeId,
    pub(super) signature: SignatureRef,
    /// The session interner the representation and every client row share.
    pub(super) types: std::sync::Arc<TypeInterner>,
}
/// Declarations `D0..Dn`; each representation names only earlier ones.
pub(super) struct World(pub(super) Vec<Declaration>);
impl World {
    pub(super) fn path(i: usize) -> DefPath {
        DefPath(hirsel_syntax::ItemPath {
            package: hirsel_syntax::PackageId {
                name: "reference".into(),
                version: "1".into(),
            },
            modules: vec![],
            name: format!("D{i}").as_str().into(),
        })
    }
    pub(super) fn get(&self, path: DefId) -> Option<&Declaration> {
        let i: usize = path.path().0.name.strip_prefix('D')?.parse().ok()?;
        self.0.get(i)
    }
    pub(super) fn generate(r: &mut Random, types: &std::sync::Arc<TypeInterner>) -> Self {
        let mut world = World(vec![]);
        for i in 0..1 + r.below(5) as usize {
            let declared = match r.below(9) {
                0 | 1 | 2 => Declared::Transparent,
                3 | 4 => Declared::Opaque(OPAQUE_BOUNDS[r.below(9) as usize]),
                5 => Declared::FlatLayout,
                6 => Declared::Resource,
                7 => Declared::GlobalCapability,
                _ => Declared::ScopedCapability,
            };
            let generics = match declared {
                Declared::FlatLayout => 0,
                _ => r.below(3) as usize,
            };
            let representation = match declared {
                Declared::FlatLayout => {
                    let fields = (0..1 + r.below(3))
                        .map(|f| {
                            let p = [Primitive::U64, Primitive::String, Primitive::Bytes]
                                [r.below(3) as usize];
                            Field {
                                growable: false,
                                name: format!("f{f}").as_str().into(),
                                ty: types.intern(Type::with_kind(TypeKind::Primitive(p))),
                            }
                        })
                        .collect::<Vec<_>>();
                    types.intern(Type::with_kind(TypeKind::Record(fields.into())))
                }
                _ => crossing_type(r, types, 2, Scope::Declaration(generics), &world),
            };
            let (bounds, visibility) = match declared {
                Declared::Opaque(bounds) => {
                    (bounds.to_vec(), hirsel_syntax::Visibility::PublicOpaque)
                }
                Declared::FlatLayout => (vec![Bound::Flat], hirsel_syntax::Visibility::Private),
                _ => (vec![], hirsel_syntax::Visibility::Private),
            };
            let signature = std::sync::Arc::new(Signature {
                definition: DefId::intern(&World::path(i)),
                metadata: SignatureMetadata {
                    visibility,
                    ..SignatureMetadata::default()
                },
                fingerprint: Default::default(),
                kind: SignatureKind::Type(Box::new(TypeDefinition {
                    loop_source: None,
                    format: None,
                    destructor_allocation: Allocation::MayAllocate,
                    generics: (0..generics)
                        .map(|g| GenericParameter::Type {
                            name: format!("U{g}").as_str().into(),
                            bounds: vec![].into(),
                        })
                        .collect(),
                    representation: (!matches!(declared, Declared::Opaque(_)))
                        .then_some(representation),
                    bounds,
                    authority: AuthoritySummary::inert(),
                    capability: match declared {
                        Declared::GlobalCapability => Some(CapabilityKind::Global),
                        Declared::ScopedCapability => Some(CapabilityKind::Scoped),
                        _ => None,
                    },
                    implementation_authority: None,
                    serializable: false,
                    serializable_check: None,
                    layout: Layout {
                        flat: matches!(declared, Declared::FlatLayout),
                        ..Layout::default()
                    },
                    distinct: false,
                    resource: matches!(declared, Declared::Resource),
                    drop: None,
                    fact: None,
                    captures: None,
                })),
            });
            world.0.push(Declaration {
                declared,
                generics,
                representation,
                signature,
                types: std::sync::Arc::clone(types),
            });
        }
        world
    }
}

/// Which parameters a generated row may name: the root scope's `T0..T9`, or
/// a declaration's own unbounded `U0..Un`.
#[derive(Clone, Copy)]
enum Scope {
    Root,
    Declaration(usize),
}

/// A crossing summary for a callable or `Iter` row.
fn summary(r: &mut Random) -> Sendability {
    match r.below(3) {
        0 => Sendability::Sendable,
        1 => Sendability::NotSendable,
        _ => Sendability::DependsOn(
            vec![TypeParameterRef::bound(TypeParamId(
                [0, 4][r.below(2) as usize],
            ))]
            .into(),
        ),
    }
}

/// A random acyclic graph. Root-scope rows draw locality, uniqueness and
/// crossing evidence at random; declaration rows keep the default modes a
/// representation is written with.
fn crossing_type(
    r: &mut Random,
    store: &TypeInterner,
    depth: u32,
    scope: Scope,
    world: &World,
) -> TypeId {
    let choice = if depth == 0 { r.below(4) } else { r.below(15) };
    let child = |r: &mut Random, store: &TypeInterner| {
        crossing_type(r, store, depth.saturating_sub(1), scope, world)
    };
    let builtin = |kind, arguments: Vec<TypeId>| TypeKind::Builtin {
        kind,
        arguments: arguments
            .into_iter()
            .map(GenericArgument::Type)
            .collect::<Vec<_>>()
            .into(),
    };
    let parameters = match scope {
        Scope::Root => CROSSING_BOUNDS.len(),
        Scope::Declaration(n) => n,
    };
    let kind = match choice {
        0 => TypeKind::Primitive(
            [
                Primitive::U8,
                Primitive::U64,
                Primitive::Bool,
                Primitive::F64,
            ][r.below(4) as usize],
        ),
        1 => TypeKind::Primitive([Primitive::String, Primitive::Bytes][r.below(2) as usize]),
        2 | 3 if parameters > 0 => TypeKind::Parameter(TypeParameterRef::bound(TypeParamId(
            r.below(parameters as u64) as u32,
        ))),
        2 | 3 => TypeKind::Tuple(vec![].into()),
        4 => {
            let unit = store.intern(Type::with_kind(TypeKind::Tuple(vec![].into())));
            TypeKind::Function(FunctionType {
                state_promises: Default::default(),
                generics: vec![].into(),
                parameters: vec![].into(),
                result: unit,
                error: None,
                allocation: Allocation::NoAlloc,
                authority: AuthoritySummary::inert(),
                uses: Uses::Only(AuthoritySummary::inert()),
                contracts: vec![].into(),
                incomplete: false,
                foreign: None,
            })
        }
        5 => TypeKind::Tuple(
            (0..r.below(4))
                .map(|_| child(r, store))
                .collect::<Vec<_>>()
                .into(),
        ),
        6 => TypeKind::Record(
            (0..1 + r.below(3))
                .map(|f| Field {
                    growable: false,
                    name: format!("f{f}").as_str().into(),
                    ty: child(r, store),
                })
                .collect::<Vec<_>>()
                .into(),
        ),
        7 => TypeKind::Variant(
            (0..1 + r.below(3))
                .map(|v| Variant {
                    name: format!("V{v}").as_str().into(),
                    fields: (0..r.below(3))
                        .map(|_| (None, child(r, store)))
                        .collect::<Vec<_>>()
                        .into(),
                })
                .collect::<Vec<_>>()
                .into(),
        ),
        8 => TypeKind::Box(child(r, store)),
        9 => match r.below(3) {
            0 => builtin(BuiltinType::List, vec![child(r, store)]),
            1 => builtin(BuiltinType::Set, vec![child(r, store)]),
            _ => builtin(BuiltinType::Map, vec![child(r, store), child(r, store)]),
        },
        10 => TypeKind::Builtin {
            kind: [BuiltinType::Inline, BuiltinType::Simd][r.below(2) as usize],
            arguments: vec![
                GenericArgument::Type(child(r, store)),
                GenericArgument::Size(Size::Constant(2)),
            ]
            .into(),
        },
        11 | 12 => {
            let kind = [
                BuiltinType::Shared,
                BuiltinType::Mutex,
                BuiltinType::Sender,
                BuiltinType::Receiver,
                BuiltinType::Task,
                BuiltinType::Atomic,
                BuiltinType::Region,
                BuiltinType::Iter,
                BuiltinType::SlotId,
                BuiltinType::Ptr,
            ][r.below(10) as usize];
            builtin(kind, vec![child(r, store)])
        }
        _ if world.0.is_empty() => TypeKind::Tuple(vec![].into()),
        _ => {
            let i = r.below(world.0.len() as u64) as usize;
            TypeKind::Named {
                definition: DefId::intern(&World::path(i)),
                arguments: (0..world.0[i].generics)
                    .map(|_| GenericArgument::Type(child(r, store)))
                    .collect::<Vec<_>>()
                    .into(),
            }
        }
    };
    let mut row = Type::with_kind(kind);
    let callable = matches!(
        row.kind,
        TypeKind::Function(_)
            | TypeKind::Builtin {
                kind: BuiltinType::Iter,
                ..
            }
    );
    match scope {
        Scope::Root => {
            if r.below(7) == 0 {
                row.modes.relation = Relation::environment();
            }
            if r.below(3) == 0 {
                row.modes.uniqueness = Uniqueness::Unique;
            }
            row.modes.sendability = if callable {
                summary(r)
            } else if r.below(5) == 0 {
                Sendability::Sendable
            } else {
                Sendability::NotSendable
            };
        }
        Scope::Declaration(_) if callable => {
            row.modes.sendability =
                [Sendability::Sendable, Sendability::NotSendable][r.below(2) as usize].clone();
        }
        Scope::Declaration(_) => (),
    }
    let id = store.intern(row.clone());
    // Normalized graphs give every row that proves plain one canonical
    // spelling of its modes: global, aliased and sendable.
    let at = At {
        store,
        id,
        actuals: None,
    };
    if matches!(scope, Scope::Root) && rich_plain(world, at) {
        row.canonicalize_plain();
        return store.intern(row);
    }
    id
}

/// A row in its store, with the actuals a declaration's parameters stand
/// for (`None` in the root scope, whose parameters are `T0..T9`).
#[derive(Clone)]
struct At<'a> {
    store: &'a TypeInterner,
    id: TypeId,
    actuals: Option<std::rc::Rc<Vec<At<'a>>>>,
}
enum Node<'a> {
    Row(At<'a>, &'a Type),
    /// A root-scope parameter, with its bounds.
    Parameter(TypeParameterRef, &'static [Bound], &'a Type),
    /// A declaration's representation, already substituted.
    Nominal(Declared, Box<At<'a>>, &'a Type),
}
impl<'a> At<'a> {
    fn part(&self, id: TypeId) -> Self {
        Self {
            store: self.store,
            id,
            actuals: self.actuals.clone(),
        }
    }
    fn parts(&self) -> Vec<Self> {
        let t = self.store.get(self.id).unwrap();
        let ids = match &t.kind {
            TypeKind::Record(fs) => fs.iter().map(|f| f.ty).collect(),
            TypeKind::Function(_) => vec![],
            _ => parts(t),
        };
        ids.into_iter().map(|id| self.part(id)).collect()
    }
    /// A declaration's parameter is its actual; a nominal row is its
    /// declaration and representation.
    fn node(&self, world: &'a World) -> Node<'a> {
        let t = self.store.get(self.id).unwrap();
        match (&t.kind, &self.actuals) {
            (TypeKind::Parameter(p), Some(actuals)) => {
                actuals[p.parameter().0 as usize].node(world)
            }
            (TypeKind::Parameter(p), None) => {
                Node::Parameter(*p, CROSSING_BOUNDS[p.parameter().0 as usize], t)
            }
            (
                TypeKind::Named {
                    definition,
                    arguments,
                },
                _,
            ) => {
                let declaration = world.get(*definition).unwrap();
                let actuals = type_arguments(arguments)
                    .into_iter()
                    .map(|id| self.part(id))
                    .collect();
                Node::Nominal(
                    declaration.declared,
                    Box::new(At {
                        store: &declaration.types,
                        id: declaration.representation,
                        actuals: Some(std::rc::Rc::new(actuals)),
                    }),
                    t,
                )
            }
            _ => Node::Row(self.clone(), t),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Crossing {
    /// `structural_sendability`: the complete value, its root lifetime too,
    /// with no occurrence evidence.
    Structural,
    /// `content_sendability`: the current referent, trusting checked
    /// occurrence evidence; the root lifetime is checked elsewhere.
    Content,
    /// `sendability_after_sharing`: the content after a counted copy, so no
    /// part is unique any more and no evidence survives.
    AfterSharing,
}

fn reference_sendability(world: &World, root: At<'_>, crossing: Crossing) -> Sendability {
    let mut obligations = BTreeSet::new();
    if sendable(world, root, true, false, crossing, &mut obligations) {
        if obligations.is_empty() {
            Sendability::Sendable
        } else {
            Sendability::DependsOn(obligations.into_iter().collect::<Vec<_>>().into())
        }
    } else {
        Sendability::NotSendable
    }
}

/// MODE-4: plain and flat values cross by copy; unique counted values whose
/// contents cross move; `Shared`, Senders and Mutexes name run-owned cells;
/// Atomics and regions cross with what they hold. Receivers, task handles,
/// scoped capabilities, aliased counted values and local values never do.
/// A unique owner's parts are unique (MODE-2).
fn sendable<'a>(
    world: &'a World,
    at: At<'a>,
    root: bool,
    unique_owner: bool,
    crossing: Crossing,
    obligations: &mut BTreeSet<TypeParameterRef>,
) -> bool {
    let node = at.node(world);
    let t = match &node {
        Node::Row(_, t) | Node::Parameter(_, _, t) | Node::Nominal(_, _, t) => *t,
    };
    let unique = crossing != Crossing::AfterSharing
        && (unique_owner || t.modes.uniqueness == Uniqueness::Unique);
    // Locality wins, even for flat values. A callable's or Iter's crossing
    // mode describes its captures, whatever the lifetime of the view.
    let callable = matches!(
        t.kind,
        TypeKind::Function(_)
            | TypeKind::Builtin {
                kind: BuiltinType::Iter,
                ..
            }
    );
    if !t.modes.relation.is_global() && (!root || crossing == Crossing::Structural && !callable) {
        return false;
    }
    if crossing == Crossing::Content && t.modes.sendability == Sendability::Sendable {
        return true;
    }
    let captures = |obligations: &mut BTreeSet<_>| match &t.modes.sendability {
        Sendability::Sendable => true,
        Sendability::NotSendable => false,
        Sendability::DependsOn(ps) => {
            obligations.extend(ps.iter().copied());
            true
        }
    };
    match node {
        Node::Parameter(p, bounds, _) => {
            let copied = bounds.contains(&Bound::Plain) || bounds.contains(&Bound::Flat);
            if copied || crossing != Crossing::AfterSharing && bounds.contains(&Bound::Sendable) {
                true
            } else if crossing == Crossing::AfterSharing {
                // M1: a share leaves no unique `T`, and only a copied one crosses.
                false
            } else {
                obligations.insert(p);
                true
            }
        }
        Node::Nominal(declared, representation, _) => match declared {
            Declared::ScopedCapability => false,
            Declared::FlatLayout => true,
            Declared::Opaque(bounds) => {
                bounds.contains(&Bound::Plain)
                    || bounds.contains(&Bound::Flat)
                    || crossing != Crossing::AfterSharing && bounds.contains(&Bound::Sendable)
            }
            Declared::Transparent | Declared::Resource | Declared::GlobalCapability => {
                sendable(world, *representation, false, unique, crossing, obligations)
            }
        },
        Node::Row(at, t) => {
            let all = |obligations: &mut BTreeSet<_>| {
                at.parts()
                    .into_iter()
                    .all(|part| sendable(world, part, false, unique, crossing, obligations))
            };
            match &t.kind {
                TypeKind::Primitive(Primitive::String | Primitive::Bytes) => unique,
                TypeKind::Primitive(_) => true,
                TypeKind::Tuple(_) | TypeKind::Record(_) | TypeKind::Variant(_) => all(obligations),
                TypeKind::Box(_) => unique && all(obligations),
                TypeKind::Function(_) => captures(obligations),
                TypeKind::Builtin { kind, .. } => match kind {
                    BuiltinType::Ptr | BuiltinType::FnPtr | BuiltinType::SlotId => true,
                    BuiltinType::Shared | BuiltinType::Sender | BuiltinType::Mutex => true,
                    BuiltinType::Receiver | BuiltinType::Task => false,
                    BuiltinType::Iter => captures(obligations),
                    // MEM-4 preserves the sealed payload's own receipt. The
                    // affine owner's uniqueness never upgrades that payload.
                    BuiltinType::Region => at
                        .parts()
                        .into_iter()
                        .all(|part| sendable(world, part, false, false, crossing, obligations)),
                    BuiltinType::Atomic | BuiltinType::Inline | BuiltinType::Simd => {
                        all(obligations)
                    }
                    _ => unique && all(obligations),
                },
                _ => false,
            }
        }
    }
}

/// GEN-1: holds no block and copies implicitly. A `flat type` is copied
/// only explicitly (MEM-12), so it is not plain.
fn rich_plain<'a>(world: &'a World, at: At<'a>) -> bool {
    match at.node(world) {
        Node::Parameter(_, bounds, _) => bounds.contains(&Bound::Plain),
        Node::Nominal(declared, representation, _) => match declared {
            Declared::FlatLayout | Declared::Resource | Declared::ScopedCapability => false,
            Declared::Opaque(bounds) => bounds.contains(&Bound::Plain),
            Declared::Transparent | Declared::GlobalCapability => {
                rich_plain(world, *representation)
            }
        },
        Node::Row(at, t) => match &t.kind {
            TypeKind::Primitive(p) => !matches!(p, Primitive::String | Primitive::Bytes),
            TypeKind::Tuple(_)
            | TypeKind::Record(_)
            | TypeKind::Variant(_)
            | TypeKind::Builtin {
                kind: BuiltinType::Inline | BuiltinType::Simd,
                ..
            } => at.parts().into_iter().all(|p| rich_plain(world, p)),
            TypeKind::Builtin {
                kind: BuiltinType::Ptr | BuiltinType::FnPtr | BuiltinType::SlotId,
                ..
            } => true,
            _ => false,
        },
    }
}

/// MEM-12: a value is flat when it opts in (a `flat type`, or a declared
/// Flat proof), or when it is plain with a pointer-free representation and
/// no capability, function, handle or resource. Symbolic and opaque types
/// need the declared proof.
fn reference_flat<'a>(world: &'a World, at: At<'a>) -> bool {
    opts_into_flat(world, at.clone()) || rich_plain(world, at.clone()) && pointer_free(world, at)
}
fn opts_into_flat<'a>(world: &'a World, at: At<'a>) -> bool {
    match at.node(world) {
        Node::Parameter(_, bounds, _) => bounds.contains(&Bound::Flat),
        Node::Nominal(Declared::FlatLayout, ..) => true,
        Node::Nominal(Declared::Opaque(bounds), ..) => bounds.contains(&Bound::Flat),
        _ => false,
    }
}
fn pointer_free<'a>(world: &'a World, at: At<'a>) -> bool {
    if opts_into_flat(world, at.clone()) {
        return true;
    }
    match at.node(world) {
        Node::Parameter(..) => false,
        Node::Nominal(declared, representation, _) => match declared {
            Declared::Transparent => pointer_free(world, *representation),
            _ => false,
        },
        Node::Row(at, t) => match &t.kind {
            TypeKind::Primitive(_) => true,
            TypeKind::Tuple(_)
            | TypeKind::Record(_)
            | TypeKind::Variant(_)
            | TypeKind::Builtin {
                kind: BuiltinType::Inline | BuiltinType::Simd,
                ..
            } => at.parts().into_iter().all(|p| pointer_free(world, p)),
            _ => false,
        },
    }
}

/// GEN-1: `data` has no functions, capabilities, resources, handles, arenas
/// or raw pointers, and `Shared`, `Slots`, `TypeShape` and `SortedMap` are
/// not data; `key` is data without floats.
fn reference_equality<'a>(world: &'a World, at: At<'a>, key: bool) -> bool {
    let declared =
        |bounds: &[Bound]| bounds.contains(&Bound::Key) || !key && bounds.contains(&Bound::Data);
    match at.node(world) {
        Node::Parameter(_, bounds, _) => declared(bounds),
        Node::Nominal(kind, representation, _) => match kind {
            Declared::Resource | Declared::GlobalCapability | Declared::ScopedCapability => false,
            Declared::Opaque(bounds) => declared(bounds),
            Declared::Transparent | Declared::FlatLayout => {
                reference_equality(world, *representation, key)
            }
        },
        Node::Row(at, t) => match &t.kind {
            TypeKind::Primitive(Primitive::F32 | Primitive::F64) => !key,
            TypeKind::Primitive(_) => true,
            TypeKind::Tuple(_)
            | TypeKind::Record(_)
            | TypeKind::Variant(_)
            | TypeKind::Box(_)
            | TypeKind::Builtin {
                kind:
                    BuiltinType::List
                    | BuiltinType::Map
                    | BuiltinType::Set
                    | BuiltinType::Inline
                    | BuiltinType::Simd,
                ..
            } => at
                .parts()
                .into_iter()
                .all(|p| reference_equality(world, p, key)),
            _ => false,
        },
    }
}

/// A failing case, spelled with its modes, and the declarations it names.
fn spell(store: &TypeInterner, id: TypeId) -> String {
    let t = store.get(id).unwrap();
    let mut out = String::new();
    if !t.modes.relation.is_global() {
        out += "local ";
    }
    if t.modes.uniqueness == Uniqueness::Unique {
        out += "unique ";
    }
    match &t.modes.sendability {
        Sendability::Sendable => out += "evidenced ",
        Sendability::DependsOn(ps) => out += &format!("depends{ps:?} "),
        Sendability::NotSendable => (),
    }
    let list = |ids: Vec<TypeId>| {
        ids.into_iter()
            .map(|id| spell(store, id))
            .collect::<Vec<_>>()
            .join(", ")
    };
    out += &match &t.kind {
        TypeKind::Primitive(p) => format!("{p:?}"),
        TypeKind::Parameter(p) => format!("T{}", p.parameter().0),
        TypeKind::Tuple(ts) => format!("({})", list(ts.to_vec())),
        TypeKind::Record(fs) => format!("{{{}}}", list(fs.iter().map(|f| f.ty).collect())),
        TypeKind::Variant(_) => format!("<{}>", list(parts(t))),
        TypeKind::Box(b) => format!("Box[{}]", spell(store, *b)),
        TypeKind::Function(_) => "fn".into(),
        TypeKind::Builtin { kind, arguments } => {
            format!("{kind:?}[{}]", list(type_arguments(arguments)))
        }
        TypeKind::Named {
            definition,
            arguments,
        } => format!(
            "{}[{}]",
            definition.path().0.name,
            list(type_arguments(arguments))
        ),
        TypeKind::Error => "error".into(),
    };
    out
}
fn describe(world: &World, store: &TypeInterner, root: TypeId) -> String {
    let mut out = spell(store, root);
    for (i, d) in world.0.iter().enumerate() {
        out += &format!(
            "\n  D{i}[{}] {:?} = {}",
            d.generics,
            d.declared,
            spell(&d.types, d.representation)
        );
    }
    out
}

#[test]
fn crossing_flatness_and_equality_agree_with_a_spec_reference() {
    let declarations = crossing_declarations();
    let generics = GenericDeclarations::signature(&declarations);
    // Per checked property, how often each verdict occurred.
    let mut verdicts = std::collections::BTreeMap::<(&str, String), usize>::new();
    // The first disagreements of each property, and how many there were.
    let mut disagreements = std::collections::BTreeMap::<&str, (usize, Vec<String>)>::new();
    let mut check =
        |property: &'static str, actual: String, expected: String, case: &dyn Fn() -> String| {
            if actual != expected {
                let (count, cases) = disagreements.entry(property).or_default();
                *count += 1;
                if cases.len() < 3 {
                    cases.push(format!(
                        "{property}: prove {actual}, reference {expected}, {}",
                        case()
                    ));
                }
            }
        };
    for seed in 1..=6000u64 {
        let mut r = Random(seed.wrapping_mul(0x2545_f491_4f6c_dd1d) | 1);
        let store = TypeInterner::new(None);
        let world = World::generate(&mut r, &store);
        let root = crossing_type(&mut r, &store, 1 + (seed % 3) as u32, Scope::Root, &world);
        let at = At {
            store: &store,
            id: root,
            actuals: None,
        };
        let mut lookup =
            |path: DefId| Ok::<_, QueryError>(world.get(path).map(|d| d.signature.clone()));
        let case = || format!("seed {seed}: {}", describe(&world, &store, root));
        for (property, crossing) in [
            ("structural sendability", Crossing::Structural),
            ("content sendability", Crossing::Content),
            ("sendability after sharing", Crossing::AfterSharing),
        ] {
            let expected = reference_sendability(&world, at.clone(), crossing);
            let actual = match crossing {
                Crossing::Structural => {
                    structural_sendability(&store, root, &generics, &mut lookup)
                }
                Crossing::Content => content_sendability(&store, root, &generics, &mut lookup),
                Crossing::AfterSharing => {
                    sendability_after_sharing(&store, root, &generics, &mut lookup)
                }
            }
            .unwrap();
            check(
                property,
                format!("{actual:?}"),
                format!("{expected:?}"),
                &case,
            );
            let verdict = match expected {
                Sendability::DependsOn(_) => "depends".to_string(),
                other => format!("{other:?}"),
            };
            *verdicts.entry((property, verdict)).or_default() += 1;
        }
        let flat = reference_flat(&world, at.clone());
        let actual = structural_flat(&store, root, &generics, &mut lookup).unwrap();
        check("flat", actual.to_string(), flat.to_string(), &case);
        *verdicts.entry(("flat", flat.to_string())).or_default() += 1;
        for (property, bound, key) in [("data", Bound::Data, false), ("key", Bound::Key, true)] {
            let expected = reference_equality(&world, at.clone(), key);
            let actual = structural_equality(&store, root, bound, &generics, &mut lookup).unwrap();
            check(property, actual.to_string(), expected.to_string(), &case);
            *verdicts
                .entry((property, expected.to_string()))
                .or_default() += 1;
        }
    }
    assert!(disagreements.is_empty(), "{disagreements:#?}");
    // Every verdict of every property occurs often enough to mean something.
    assert_eq!(verdicts.len(), 3 * 3 + 3 * 2, "{verdicts:?}");
    assert!(verdicts.values().all(|n| *n > 100), "{verdicts:?}");
}
