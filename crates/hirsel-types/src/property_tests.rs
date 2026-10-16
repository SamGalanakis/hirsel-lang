//! `prove` decides Plain, Copyable and NeedsRelease for J, V and RC alike,
//! so their agreement campaign cannot see a bug in it. This differential
//! checks it against a naive recursive reference written from GEN-1: plain is
//! "copyable and holds no blocks"; copying needs every owned part copyable and
//! is never available for a `once` value (MODE-3); release is needed exactly
//! when a value is not plain (DROP-1/MEM-1). The graphs are random but
//! deterministic, so a failure names a seed that reproduces it.
use super::*;

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
        name: format!("T{i}"),
        bounds: bounds.into(),
    })
    .collect()
}

struct Random(u64);
impl Random {
    fn below(&mut self, n: u64) -> u64 {
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
fn generate(r: &mut Random, store: &mut TypeStore, depth: u32) -> TypeId {
    let leaf = depth == 0;
    let choice = if leaf { r.below(5) } else { r.below(12) };
    let mut child = |r: &mut Random, store: &mut TypeStore| generate(r, store, depth - 1);
    let kind = match choice {
        0 => TypeKind::Primitive(
            [Primitive::U8, Primitive::U64, Primitive::Bool][r.below(3) as usize],
        ),
        1 => TypeKind::Primitive([Primitive::String, Primitive::Bytes][r.below(2) as usize]),
        2 => TypeKind::Parameter(TypeParameterRef::bound(TypeParamId(r.below(4) as u32))),
        3 => {
            let mut f = Type::boolean();
            f.kind = TypeKind::Tuple(vec![].into());
            let unit = store.push(f);
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
                    name: format!("V{v}"),
                    fields: (0..r.below(3))
                        .map(|i| (Some(format!("f{i}")), child(r, store)))
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
    store.push(row(kind, once))
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

fn reference_copyable(store: &TypeStore, id: TypeId) -> bool {
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

fn reference_plain(store: &TypeStore, id: TypeId) -> bool {
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
    let mut lookup = |_: &DefPath| Ok::<_, QueryError>(None);
    let mut counts = [0usize; 2];
    for seed in 1..=4000u64 {
        let mut r = Random(seed.wrapping_mul(0x9e37_79b9_7f4a_7c15));
        let mut store = TypeStore::default();
        let root = generate(&mut r, &mut store, 1 + (seed % 4) as u32);
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
    let mut lookup = |_: &DefPath| Ok::<_, QueryError>(None);
    for seed in 1..=2000u64 {
        let mut r = Random(seed.wrapping_mul(0xd1b5_4a32_d192_ed03));
        let mut store = TypeStore::default();
        let base = generate(&mut r, &mut store, 2);
        let added = generate(&mut r, &mut store, 2);
        let before = store.push(row(TypeKind::Tuple(vec![base].into()), false));
        let after = store.push(row(TypeKind::Tuple(vec![base, added].into()), false));
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
