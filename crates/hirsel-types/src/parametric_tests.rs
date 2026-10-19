//! `parametric_inputs` and `unique_rows` decide MODE-2's parametric calls for
//! both I and V, so a fault in them is common to both checkers. This
//! differential checks them against a reference written from MODE-2's
//! parametric rule and GEN-1's linear type parameters, over random generic
//! signatures whose parameters, callbacks and nominal parts name the
//! result's type parameters in each position the rule distinguishes. The
//! signatures are random but deterministic, so a failure names its seed.
use super::property_tests::{Declared, Random, World};
use super::*;
use hirsel_syntax::PassingMode;
use std::{collections::BTreeSet, rc::Rc};

/// Bound sets a signature's parameters draw from; unbounded is drawn twice,
/// since only parameters without copying permission are held.
const BOUND_SETS: [&[Bound]; 7] = [
    &[],
    &[],
    &[Bound::Copyable],
    &[Bound::Plain],
    &[Bound::Sendable],
    &[Bound::Data],
    &[Bound::Flat],
];

/// GEN-1: these bounds let a body copy a value of the parameter.
fn copying(bounds: &[Bound]) -> bool {
    bounds.iter().any(|b| {
        matches!(
            b,
            Bound::Copyable | Bound::Data | Bound::Key | Bound::Flat | Bound::Plain
        )
    })
}

/// A random type of a signature, `binder` function binders below the
/// signature's own generics, which it names often.
fn signature_type(
    r: &mut Random,
    store: &mut TypeStore,
    depth: u32,
    binder: u32,
    generics: usize,
    world: &World,
) -> TypeId {
    let choice = if depth == 0 { r.below(6) } else { r.below(16) };
    let mut child = |r: &mut Random, store: &mut TypeStore| {
        signature_type(r, store, depth - 1, binder, generics, world)
    };
    let builtin = |kind, arguments: Vec<TypeId>| TypeKind::Builtin {
        kind,
        arguments: arguments
            .into_iter()
            .map(GenericArgument::Type)
            .collect::<Vec<_>>()
            .into(),
    };
    let kind = match choice {
        0..=3 => TypeKind::Parameter(TypeParameterRef::Bound {
            depth: binder,
            parameter: TypeParamId(r.below(generics as u64) as u32),
        }),
        4 => TypeKind::Primitive(Primitive::U64),
        5 => TypeKind::Primitive(Primitive::String),
        6 | 7 => TypeKind::Tuple(
            (0..1 + r.below(3))
                .map(|_| child(r, store))
                .collect::<Vec<_>>()
                .into(),
        ),
        8 => TypeKind::Variant(
            vec![
                Variant {
                    name: "Some".into(),
                    fields: vec![(None, child(r, store))].into(),
                },
                Variant {
                    name: "None".into(),
                    fields: vec![].into(),
                },
            ]
            .into(),
        ),
        9 => TypeKind::Record(
            (0..1 + r.below(2))
                .map(|f| Field {
                    growable: false,
                    name: format!("f{f}"),
                    ty: child(r, store),
                })
                .collect::<Vec<_>>()
                .into(),
        ),
        10 => builtin(BuiltinType::List, vec![child(r, store)]),
        11 => TypeKind::Box(child(r, store)),
        12 => TypeKind::Builtin {
            kind: BuiltinType::Inline,
            arguments: vec![
                GenericArgument::Type(child(r, store)),
                GenericArgument::Size(Size::Constant(2)),
            ]
            .into(),
        },
        13 => {
            let kind = [
                BuiltinType::Shared,
                BuiltinType::Mutex,
                BuiltinType::Atomic,
                BuiltinType::Iter,
                BuiltinType::Sender,
            ][r.below(5) as usize];
            builtin(kind, vec![child(r, store)])
        }
        _ if world.0.is_empty() => TypeKind::Primitive(Primitive::U64),
        _ => {
            let i = r.below(world.0.len() as u64) as usize;
            TypeKind::Named {
                definition: World::path(i),
                arguments: (0..world.0[i].generics)
                    .map(|_| GenericArgument::Type(child(r, store)))
                    .collect::<Vec<_>>()
                    .into(),
            }
        }
    };
    let mut row = Type::with_kind(kind);
    if r.below(4) == 0 {
        row.modes.uniqueness = Uniqueness::Unique;
    }
    store.push(row)
}

const PASSING: [PassingMode; 3] = [PassingMode::Take, PassingMode::Borrow, PassingMode::Mut];

/// A callback parameter: what it receives, what it returns, and its `uses`.
fn callback(r: &mut Random, store: &mut TypeStore, generics: usize, world: &World) -> TypeId {
    let parameters = (0..r.below(3))
        .map(|_| Parameter {
            name: None,
            label: None,
            passing: PASSING[r.below(3) as usize],
            ty: {
                let depth = r.below(2) as u32;
                signature_type(r, store, depth, 1, generics, world)
            },
        })
        .collect::<Vec<_>>();
    let depth = r.below(3) as u32;
    let result = signature_type(r, store, depth, 1, generics, world);
    let error = (r.below(4) == 0).then(|| signature_type(r, store, 0, 1, generics, world));
    store.push(Type::with_kind(TypeKind::Function(FunctionType {
        state_promises: Default::default(),
        generics: vec![].into(),
        parameters: parameters.into(),
        result,
        error,
        allocation: Allocation::MayAllocate,
        authority: AuthoritySummary::inert(),
        uses: if r.below(3) == 0 {
            Uses::Unrestricted
        } else {
            Uses::Only(AuthoritySummary::inert())
        },
        contracts: vec![].into(),
        incomplete: false,
        foreign: None,
    })))
}

fn signature(r: &mut Random, store: &mut TypeStore, world: &World) -> FunctionType {
    let generics = 1 + r.below(3) as usize;
    let parameters = (0..1 + r.below(4))
        .map(|_| Parameter {
            name: None,
            label: None,
            passing: PASSING[r.below(3) as usize],
            ty: if r.below(4) == 0 {
                callback(r, store, generics, world)
            } else {
                let depth = r.below(3) as u32;
                signature_type(r, store, depth, 0, generics, world)
            },
        })
        .collect::<Vec<_>>();
    let depth = r.below(3) as u32;
    let result = signature_type(r, store, depth, 0, generics, world);
    let mut result = store.get(result).unwrap().clone();
    if r.below(8) == 0 {
        result.modes.relation = Relation::parameters([ParameterRef::bound(ParamId(0))]);
    }
    let result = store.push(result);
    FunctionType {
        state_promises: Default::default(),
        generics: (0..generics)
            .map(|i| GenericParameter::Type {
                name: format!("T{i}"),
                bounds: BOUND_SETS[r.below(BOUND_SETS.len() as u64) as usize]
                    .to_vec()
                    .into(),
            })
            .collect::<Vec<_>>()
            .into(),
        parameters: parameters.into(),
        result,
        error: None,
        allocation: Allocation::MayAllocate,
        authority: AuthoritySummary::inert(),
        uses: Uses::Unrestricted,
        contracts: vec![].into(),
        incomplete: false,
        foreign: None,
    }
}

/// A row of the signature, or of a declaration's representation with the
/// signature rows its parameters stand for. `binder` counts the function
/// binders between the row and the signature's generics.
#[derive(Clone)]
struct Place<'a> {
    store: &'a TypeStore,
    id: TypeId,
    binder: u32,
    actuals: Option<Rc<Vec<Place<'a>>>>,
}
enum Seen<'a> {
    /// One of the signature's own type parameters.
    Generic(u32),
    /// A parameter of a nested callable's own binder, which the signature
    /// does not instantiate.
    Inner,
    Nominal(Declared, Place<'a>),
    Row(&'a Type, Place<'a>),
}
impl<'a> Place<'a> {
    fn at(&self, id: TypeId, binder: u32) -> Self {
        Self {
            store: self.store,
            id,
            binder,
            actuals: self.actuals.clone(),
        }
    }
    /// A declaration's parameter stands for its actual.
    fn resolved(&self) -> Self {
        match (&self.store.get(self.id).unwrap().kind, &self.actuals) {
            (TypeKind::Parameter(p), Some(actuals)) => actuals[p.parameter().0 as usize].resolved(),
            _ => self.clone(),
        }
    }
    fn see(&self, world: &'a World) -> Seen<'a> {
        let t = self.store.get(self.id).unwrap();
        match (&t.kind, &self.actuals) {
            (TypeKind::Parameter(p), Some(actuals)) => actuals[p.parameter().0 as usize].see(world),
            (TypeKind::Parameter(p), None) if p.binder() == GenericBinder::Bound(self.binder) => {
                Seen::Generic(p.parameter().0)
            }
            (TypeKind::Parameter(_), None) => Seen::Inner,
            (
                TypeKind::Named {
                    definition,
                    arguments,
                },
                _,
            ) => {
                let declaration = world.get(definition).unwrap();
                let actuals = arguments
                    .iter()
                    .filter_map(|a| match a {
                        GenericArgument::Type(id) => Some(self.at(*id, self.binder)),
                        GenericArgument::Size(_) => None,
                    })
                    .collect();
                Seen::Nominal(
                    declaration.declared,
                    Place {
                        store: &declaration.signature.types,
                        id: declaration.representation,
                        binder: 0,
                        actuals: Some(Rc::new(actuals)),
                    },
                )
            }
            _ => Seen::Row(t, self.clone()),
        }
    }
    /// Owned parts, and a callable's parameter, result and error rows one
    /// binder deeper.
    fn edges(&self) -> Vec<Self> {
        let t = self.store.get(self.id).unwrap();
        let ids: Vec<TypeId> = match &t.kind {
            TypeKind::Tuple(ts) => ts.to_vec(),
            TypeKind::Record(fs) => fs.iter().map(|f| f.ty).collect(),
            TypeKind::Variant(vs) => vs
                .iter()
                .flat_map(|v| v.fields.iter().map(|f| f.1))
                .collect(),
            TypeKind::Box(t) => vec![*t],
            TypeKind::Builtin { arguments, .. } | TypeKind::Named { arguments, .. } => arguments
                .iter()
                .filter_map(|a| match a {
                    GenericArgument::Type(t) => Some(*t),
                    GenericArgument::Size(_) => None,
                })
                .collect(),
            TypeKind::Function(f) => {
                return f
                    .parameters
                    .iter()
                    .map(|p| p.ty)
                    .chain([f.result])
                    .chain(f.error)
                    .map(|id| self.at(id, self.binder + 1))
                    .collect();
            }
            _ => vec![],
        };
        ids.into_iter().map(|id| self.at(id, self.binder)).collect()
    }
}

/// The signature's parameters `place` names, at any depth. A nominal names
/// what its arguments name.
fn named<'a>(world: &'a World, place: &Place<'a>, out: &mut BTreeSet<u32>) {
    let place = place.resolved();
    match place.see(world) {
        Seen::Generic(i) => {
            out.insert(i);
        }
        Seen::Inner => (),
        Seen::Nominal(..) | Seen::Row(..) => {
            for edge in place.edges() {
                named(world, &edge, out);
            }
        }
    }
}
fn names<'a>(world: &'a World, place: &Place<'a>, held: &BTreeSet<u32>) -> bool {
    let mut found = BTreeSet::new();
    named(world, place, &mut found);
    !found.is_disjoint(held)
}

/// Whether a value of `place` holds no counted content except values of
/// held parameters: plain (GEN-1), counting a held parameter as plain.
fn plain_but_held<'a>(
    world: &'a World,
    place: &Place<'a>,
    generics: &[GenericParameter],
    held: &BTreeSet<u32>,
) -> bool {
    match place.see(world) {
        Seen::Generic(i) => {
            held.contains(&i)
                || matches!(&generics[i as usize], GenericParameter::Type { bounds, .. } if bounds.contains(&Bound::Plain))
        }
        Seen::Inner => false,
        Seen::Nominal(declared, representation) => match declared {
            Declared::Opaque(bounds) => bounds.contains(&Bound::Plain),
            Declared::Transparent | Declared::GlobalCapability => {
                plain_but_held(world, &representation, generics, held)
            }
            Declared::FlatLayout | Declared::Resource | Declared::ScopedCapability => false,
        },
        Seen::Row(t, place) => match &t.kind {
            TypeKind::Primitive(p) => !matches!(p, Primitive::String | Primitive::Bytes),
            TypeKind::Tuple(_)
            | TypeKind::Record(_)
            | TypeKind::Variant(_)
            | TypeKind::Builtin {
                kind: BuiltinType::Inline | BuiltinType::Simd,
                ..
            } => place
                .edges()
                .iter()
                .all(|part| plain_but_held(world, part, generics, held)),
            // Pointer and slot bits are plain; their pointees are not owned.
            TypeKind::Builtin {
                kind: BuiltinType::SlotId | BuiltinType::Ptr | BuiltinType::FnPtr,
                ..
            } => true,
            _ => false,
        },
    }
}

/// Every part of `place` on the way to a held value is data: a tuple,
/// record, variant, box or counted collection. A handle, iterator or
/// function that holds one, or an opaque nominal type naming one, could hand
/// it over. An `Inline` holding one is not counted as data either, which the
/// spec's list (handles, iterators, functions) would allow: conservative.
fn data_only<'a>(world: &'a World, place: &Place<'a>, held: &BTreeSet<u32>) -> bool {
    if !names(world, place, held) {
        return true;
    }
    match place.see(world) {
        Seen::Generic(_) | Seen::Inner => true,
        Seen::Nominal(Declared::Opaque(_), _) => false,
        // The declaration's parts that name a held value must be data, and so
        // must each argument naming one, even one the representation ignores
        // (conservative, as `parametric_inputs` documents).
        Seen::Nominal(_, representation) => {
            data_only(world, &representation, held)
                && place
                    .resolved()
                    .edges()
                    .iter()
                    .all(|argument| data_only(world, argument, held))
        }
        Seen::Row(t, place) => match &t.kind {
            TypeKind::Tuple(_)
            | TypeKind::Record(_)
            | TypeKind::Variant(_)
            | TypeKind::Box(_)
            | TypeKind::Builtin {
                kind: BuiltinType::List | BuiltinType::Map | BuiltinType::Set | BuiltinType::Array,
                ..
            } => place
                .edges()
                .iter()
                .all(|part| data_only(world, part, held)),
            _ => false,
        },
    }
}

/// Names no type parameter of any binder, and holds no counted content.
fn closed_plain<'a>(world: &'a World, place: &Place<'a>) -> bool {
    let mut found = BTreeSet::new();
    named(world, place, &mut found);
    found.is_empty()
        && !mentions_inner(world, place)
        && plain_but_held(world, place, &[], &BTreeSet::new())
}
fn mentions_inner<'a>(world: &'a World, place: &Place<'a>) -> bool {
    let place = place.resolved();
    match place.see(world) {
        Seen::Inner => true,
        Seen::Generic(_) => false,
        Seen::Nominal(..) | Seen::Row(..) => place.edges().iter().any(|e| mentions_inner(world, e)),
    }
}

/// MODE-2's parametric rule, read off the signature.
fn reference_inputs(
    world: &World,
    store: &TypeStore,
    function: &FunctionType,
    checked: bool,
) -> Option<Vec<ParametricInput>> {
    let top = |id| Place {
        store,
        id,
        binder: 0,
        actuals: None,
    };
    let result = top(function.result);
    // The owned result: a result related to an argument is not one.
    if !store.get(function.result)?.modes.relation.is_global() {
        return None;
    }
    // The result holds only values of parameters the body has no permission
    // to copy (GEN-1), and no other counted content. A result that names a
    // parameter with that permission, even a plain one, is ordinary: the
    // spec's "no counted content except" would admit a plain one, a
    // conservative difference this reference keeps (reported, not a fault).
    let mut held = BTreeSet::new();
    named(world, &result, &mut held);
    if held.is_empty()
        || held.iter().any(|i| matches!(&function.generics[*i as usize], GenericParameter::Type { bounds, .. } if copying(bounds)))
        || !plain_but_held(world, &result, &function.generics, &held)
    {
        return None;
    }
    let mut inputs = vec![];
    for parameter in function.parameters.iter() {
        let place = top(parameter.ty);
        if !names(world, &place, &held) {
            inputs.push(ParametricInput::Free);
            continue;
        }
        if let TypeKind::Function(callback) = &store.get(parameter.ty)?.kind {
            let inner = |id| place.at(id, 1);
            let mut lent = false;
            for p in callback.parameters.iter() {
                if !names(world, &inner(p.ty), &held) {
                    continue;
                }
                if !data_only(world, &inner(p.ty), &held) {
                    return None;
                }
                match p.passing {
                    PassingMode::Take => (),
                    PassingMode::Borrow => lent = true,
                    // The callback writes a held value back: it is lent one.
                    PassingMode::Mut => return None,
                }
            }
            if lent {
                // It has nowhere to keep a copy of what it is lent.
                let nowhere = matches!(&callback.uses, Uses::Only(AuthoritySummary::Known(terms)) if terms.is_empty())
                    && closed_plain(world, &inner(callback.result))
                    && callback
                        .error
                        .is_none_or(|e| closed_plain(world, &inner(e)))
                    && callback
                        .parameters
                        .iter()
                        .filter(|p| p.passing == PassingMode::Mut)
                        .all(|p| closed_plain(world, &inner(p.ty)));
                if !nowhere {
                    return None;
                }
                inputs.push(ParametricInput::Free);
                continue;
            }
            if !data_only(world, &inner(callback.result), &held) {
                return None;
            }
            inputs.push(ParametricInput::UniqueResult);
            continue;
        }
        if !data_only(world, &place, &held) {
            return None;
        }
        inputs.push(match parameter.passing {
            // A checked body cannot move a held value out of a borrow, copy
            // one, or return what it derives from one. A bodyless declaration
            // may keep or hand back a share of it.
            PassingMode::Borrow if checked => ParametricInput::Free,
            PassingMode::Borrow => return None,
            PassingMode::Take | PassingMode::Mut => ParametricInput::Unique,
        });
    }
    Some(inputs)
}

#[test]
fn parametric_inputs_agree_with_a_spec_reference() {
    let mut verdicts = std::collections::BTreeMap::<String, usize>::new();
    let mut disagreements = vec![];
    let mut total = 0;
    for seed in 1..=8000u64 {
        let mut r = Random(seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1);
        let world = World::generate(&mut r);
        let mut store = TypeStore::default();
        let function = signature(&mut r, &mut store, &world);
        let checked = r.below(2) == 0;
        let mut lookup =
            |path: &DefPath| Ok::<_, QueryError>(world.get(path).map(|d| d.signature.clone()));
        let actual = parametric_inputs(&store, &function, checked, &mut lookup);
        let expected = reference_inputs(&world, &store, &function, checked);
        match &expected {
            None => *verdicts.entry("ordinary".into()).or_default() += 1,
            Some(inputs) => {
                for input in inputs {
                    *verdicts.entry(format!("{input:?}")).or_default() += 1;
                }
            }
        }
        if actual != expected {
            total += 1;
            if disagreements.len() < 6 {
                disagreements.push(format!(
                    "seed {seed} (checked {checked}): prove {actual:?}, reference {expected:?}\n{}",
                    describe(&world, &store, &function)
                ));
            }
        }
    }
    assert!(
        disagreements.is_empty(),
        "{total} disagreements:\n{}",
        disagreements.join("\n")
    );
    assert!(verdicts.values().all(|n| *n > 200), "{verdicts:?}");
    assert_eq!(verdicts.len(), 4, "{verdicts:?}");
}

/// MODE-2: a callback's instantiated result proves uniqueness by its rows:
/// written `unique`, or a tuple each of whose parts does or is plain.
#[test]
fn unique_rows_agree_with_a_spec_reference() {
    fn reference(store: &TypeStore, id: TypeId) -> bool {
        let t = store.get(id).unwrap();
        t.modes.uniqueness == Uniqueness::Unique
            || matches!(&t.kind, TypeKind::Tuple(parts) if parts.iter().all(|p| {
                reference(store, *p) || plain(store, *p)
            }))
    }
    fn plain(store: &TypeStore, id: TypeId) -> bool {
        let t = store.get(id).unwrap();
        match &t.kind {
            TypeKind::Primitive(p) => !matches!(p, Primitive::String | Primitive::Bytes),
            TypeKind::Tuple(ts) => ts.iter().all(|t| plain(store, *t)),
            TypeKind::Record(fs) => fs.iter().all(|f| plain(store, f.ty)),
            TypeKind::Variant(vs) => vs
                .iter()
                .all(|v| v.fields.iter().all(|f| plain(store, f.1))),
            TypeKind::Builtin {
                kind: BuiltinType::Inline,
                arguments,
            } => arguments.iter().all(|a| match a {
                GenericArgument::Type(t) => plain(store, *t),
                GenericArgument::Size(_) => true,
            }),
            // Only T1 is declared plain.
            TypeKind::Parameter(p) => p.parameter().0 == 1,
            _ => false,
        }
    }
    let generics = [
        GenericParameter::Type {
            name: "T0".into(),
            bounds: vec![].into(),
        },
        GenericParameter::Type {
            name: "T1".into(),
            bounds: vec![Bound::Plain].into(),
        },
    ];
    let generics = GenericDeclarations::signature(&generics);
    let world = World(vec![]);
    let mut counts = [0usize; 2];
    for seed in 1..=4000u64 {
        let mut r = Random(seed.wrapping_mul(0xd1b5_4a32_d192_ed03) | 1);
        let mut store = TypeStore::default();
        let root = signature_type(&mut r, &mut store, 1 + (seed % 3) as u32, 0, 2, &world);
        let expected = reference(&store, root);
        counts[usize::from(expected)] += 1;
        assert_eq!(
            unique_rows(&store, root, &generics, &mut |_| Ok(None)),
            expected,
            "seed {seed}: {:?}",
            store.get(root).unwrap()
        );
    }
    assert!(counts.iter().all(|n| *n > 500), "{counts:?}");
}

fn describe(world: &World, store: &TypeStore, function: &FunctionType) -> String {
    fn spell(store: &TypeStore, id: TypeId) -> String {
        let t = store.get(id).unwrap();
        let mut out = String::new();
        if !t.modes.relation.is_global() {
            out += "related ";
        }
        if t.modes.uniqueness == Uniqueness::Unique {
            out += "unique ";
        }
        let list = |ids: Vec<TypeId>| {
            ids.into_iter()
                .map(|id| spell(store, id))
                .collect::<Vec<_>>()
                .join(", ")
        };
        let arguments = |a: &[GenericArgument]| {
            list(
                a.iter()
                    .filter_map(|a| match a {
                        GenericArgument::Type(t) => Some(*t),
                        GenericArgument::Size(_) => None,
                    })
                    .collect(),
            )
        };
        out += &match &t.kind {
            TypeKind::Primitive(p) => format!("{p:?}"),
            TypeKind::Parameter(p) => match p {
                BinderRef::Bound { depth, parameter } => format!("T{}^{depth}", parameter.0),
                _ => "T?".into(),
            },
            TypeKind::Tuple(ts) => format!("({})", list(ts.to_vec())),
            TypeKind::Record(fs) => format!("{{{}}}", list(fs.iter().map(|f| f.ty).collect())),
            TypeKind::Variant(vs) => format!(
                "<{}>",
                list(
                    vs.iter()
                        .flat_map(|v| v.fields.iter().map(|f| f.1))
                        .collect()
                )
            ),
            TypeKind::Box(b) => format!("Box[{}]", spell(store, *b)),
            TypeKind::Function(f) => format!(
                "fn({}) -> {}{} uses {:?}",
                f.parameters
                    .iter()
                    .map(|p| format!("{:?} {}", p.passing, spell(store, p.ty)))
                    .collect::<Vec<_>>()
                    .join(", "),
                spell(store, f.result),
                f.error
                    .map(|e| format!(" ! {}", spell(store, e)))
                    .unwrap_or_default(),
                f.uses
            ),
            TypeKind::Builtin { kind, arguments: a } => format!("{kind:?}[{}]", arguments(a)),
            TypeKind::Named {
                definition,
                arguments: a,
            } => format!("{}[{}]", definition.0.name, arguments(a)),
            TypeKind::Error => "error".into(),
        };
        out
    }
    let mut out = format!(
        "  fn[{}]({}) -> {}",
        function
            .generics
            .iter()
            .map(|g| match g {
                GenericParameter::Type { name, bounds } => format!("{name}: {bounds:?}"),
                other => format!("{other:?}"),
            })
            .collect::<Vec<_>>()
            .join(", "),
        function
            .parameters
            .iter()
            .map(|p| format!("{:?} {}", p.passing, spell(store, p.ty)))
            .collect::<Vec<_>>()
            .join(", "),
        spell(store, function.result)
    );
    for (i, d) in world.0.iter().enumerate() {
        out += &format!(
            "\n  D{i}[{}] {:?} = {}",
            d.generics,
            d.declared,
            spell(&d.signature.types, d.representation)
        );
    }
    out
}
