//! The pool differential on random graphs: interned normal forms and
//! `ContextMemo` against main's walks (`reference`), over pairs of graphs that
//! differ in a few rows' modes, names, captures, brands or constructors.
//! Every comparison goes through `test_api`, which asserts the two agree.
use crate::declarations::tests::{def, nominal, path};
use crate::test_api::*;
use crate::*;
use hirsel_syntax::{Bound, LexicalOwner, PassingMode};
use std::sync::Arc;

struct Random(u64);
impl Random {
    fn below(&mut self, n: u64) -> u64 {
        // xorshift64*, as in the property tests.
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d) % n
    }
    fn chance(&mut self, n: u64) -> bool {
        self.below(n) == 0
    }
}

fn origin(local: u32) -> Origin {
    Origin {
        root: OriginRoot::Local(LexicalOwner::first(&path("f")), LocalId(local)),
        fields: vec![].into(),
    }
}

/// One row's facts drawn at random; `b` rows redraw some of them.
fn decorate(r: &mut Random, row: &mut Type) {
    row.modes.uniqueness = [Uniqueness::Aliased, Uniqueness::Unique][r.below(2) as usize];
    row.modes.multiplicity = [Multiplicity::Many, Multiplicity::Once][usize::from(r.chance(5))];
    if r.chance(4) {
        row.modes.relation = Relation::place(origin(r.below(2) as u32));
    }
    row.modes.sendability = match r.below(3) {
        0 => Sendability::Sendable,
        1 => Sendability::NotSendable,
        _ => Sendability::DependsOn(vec![TypeParameterRef::bound(TypeParamId(0))].into()),
    };
    if r.chance(4) {
        row.sendability_requirement = SendabilityRequirement::Sendable;
    }
    row.drop_allocation = if r.chance(2) {
        DropAllocation::NoAlloc
    } else {
        DropAllocation::MayAllocate
    };
    row.erased_captures = match r.below(5) {
        0 => Some(ErasedCaptures::Known(AuthoritySummary::inert())),
        1 => Some(ErasedCaptures::Known(AuthoritySummary::Unknown)),
        2 => Some(ErasedCaptures::FromArguments(
            // Unsorted and repeated: normal forms sort and deduplicate.
            vec![
                ParameterRef::bound(ParamId(1)).into(),
                ParameterRef::bound(ParamId(0)).into(),
                ParameterRef::bound(ParamId(1)).into(),
            ]
            .into(),
        )),
        _ => None,
    };
    if r.chance(5) {
        row.brand.alternatives.push(origin(r.below(2) as u32));
    }
}

fn kind(
    r: &mut Random,
    t: &TypeInterner,
    depth: u32,
    children: &mut dyn FnMut(&mut Random) -> TypeId,
) -> TypeKind {
    let choice = r.below(if depth == 0 { 4 } else { 12 });
    match choice {
        0 => TypeKind::Primitive([Primitive::U8, Primitive::String][r.below(2) as usize]),
        1 => TypeKind::Parameter(TypeParameterRef::bound(TypeParamId(r.below(2) as u32))),
        2 => TypeKind::Tuple(vec![].into()),
        3 => TypeKind::Primitive(Primitive::Bool),
        4 => TypeKind::Tuple(vec![children(r), children(r)].into()),
        5 => TypeKind::Record(
            vec![Field {
                name: ["x", "y"][r.below(2) as usize].into(),
                ty: children(r),
                growable: r.chance(3),
            }]
            .into(),
        ),
        6 => TypeKind::Variant(
            vec![Variant {
                name: "Some".into(),
                fields: vec![(None, children(r))].into(),
            }]
            .into(),
        ),
        7 => TypeKind::Box(children(r)),
        8 | 9 => {
            let parameters = (0..r.below(3))
                .map(|_| Parameter {
                    name: Some(["x", "y"][r.below(2) as usize].into()),
                    label: None,
                    passing: [PassingMode::Borrow, PassingMode::Mut, PassingMode::Take]
                        [r.below(3) as usize],
                    ty: children(r),
                })
                .collect::<Vec<_>>();
            let result = children(r);
            let error = r.chance(3).then(|| children(r));
            TypeKind::Function(FunctionType {
                state_promises: Default::default(),
                generics: vec![].into(),
                parameters: parameters.into(),
                result,
                error,
                allocation: [Allocation::NoAlloc, Allocation::MayAllocate][r.below(2) as usize],
                authority: AuthoritySummary::inert(),
                uses: Uses::Unrestricted,
                contracts: vec![].into(),
                incomplete: false,
                foreign: None,
            })
        }
        10 => TypeKind::Builtin {
            kind: [BuiltinType::List, BuiltinType::Iter, BuiltinType::Shared][r.below(3) as usize],
            arguments: vec![GenericArgument::Type(children(r))].into(),
        },
        _ => {
            let _ = t;
            TypeKind::Named {
                definition: def(["Wrap", "Opaque", "Missing"][r.below(3) as usize]),
                arguments: vec![GenericArgument::Type(children(r))].into(),
            }
        }
    }
}

/// A pair of graphs with the same skeleton where some rows differ.
fn pair(r: &mut Random, t: &TypeInterner, depth: u32) -> (TypeId, TypeId) {
    if depth > 0 && r.chance(12) {
        // An unrelated replacement.
        return (single(r, t, depth), single(r, t, depth));
    }
    let mut parts = vec![];
    let a_kind = kind(r, t, depth, &mut |r| {
        let (a, b) = pair(r, t, depth - 1);
        parts.push(b);
        a
    });
    let mut b_parts = parts.into_iter();
    let mut b_kind = a_kind.map_edges(|_| b_parts.next().expect("one b part per a part"));
    if let TypeKind::Function(f) = &mut b_kind
        && r.chance(3)
    {
        // Parameter names never matter; passing conventions do.
        let mut parameters = f.parameters.to_vec();
        for p in &mut parameters {
            p.name = Some("renamed".into());
            if r.chance(4) {
                p.passing = PassingMode::Borrow;
            }
        }
        f.parameters = parameters.into();
    }
    let mut a = Type::with_kind(a_kind);
    decorate(r, &mut a);
    let mut b = Type::with_kind(b_kind);
    b.modes = a.modes.clone();
    b.sendability_requirement = a.sendability_requirement.clone();
    b.drop_allocation = a.drop_allocation.clone();
    b.erased_captures = a.erased_captures.clone();
    b.brand = a.brand.clone();
    if r.chance(3) {
        decorate(r, &mut b);
    }
    (t.intern(a), t.intern(b))
}

fn single(r: &mut Random, t: &TypeInterner, depth: u32) -> TypeId {
    let kind = kind(r, t, depth, &mut |r| single(r, t, depth - 1));
    let mut row = Type::with_kind(kind);
    decorate(r, &mut row);
    t.intern(row)
}

#[test]
fn interned_equality_and_context_memo_relations_agree_with_mains_walks() {
    let generics = GenericDeclarations::signature(&[
        GenericParameter::Type {
            name: "T".into(),
            bounds: vec![].into(),
        },
        GenericParameter::Type {
            name: "U".into(),
            bounds: vec![Bound::Plain].into(),
        },
    ]);
    let (mut equal, mut compatible, mut holes) = (0, 0, 0);
    for seed in 1..=6000u64 {
        let mut r = Random(seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1);
        let t = TypeInterner::new(None);
        let world = world(&t);
        let mut lookup =
            |d: DefId| Ok::<_, QueryError>(world.iter().find(|s| s.definition == d).cloned());
        let (a, b) = pair(&mut r, &t, 4);
        for (x, y) in [(a, b), (b, a), (a, a)] {
            equal += usize::from(semantic_equal(&t, x, y).unwrap_or(false));
            for relation in [
                TypeRelation::Exact,
                TypeRelation::Value,
                TypeRelation::Parameter,
            ] {
                compatible += usize::from(
                    compatible_type(&t, x, y, relation, &generics, &mut lookup).unwrap_or(false),
                );
                let _ = compatible_view_type_in(&t, x, y, relation, &generics, &mut lookup);
            }
            let _ = compatible_handback_type(&t, x, y, &generics, &mut lookup);
            let _ = compatible_storage_type(&t, x, y, &generics, &mut lookup);
            // Caller-designated holes: a parameter of either binder.
            let hole = |kind: &TypeKind| matches!(kind, TypeKind::Parameter(p) if p.parameter() == TypeParamId(r.0 as u32 & 1));
            let new = crate::graph::shape_equal_with_holes(&t, x, y, hole);
            let old = crate::reference::shape_equal_with_holes(&t, x, &t, y, hole);
            assert_eq!(new, old, "seed {seed}: shape holes {x:?} {y:?}");
            holes += usize::from(new.unwrap_or(false));
        }
    }
    // Every relation is exercised in both verdicts.
    assert!(
        equal > 6000 && compatible > 12000 && holes > 6000,
        "{equal} {compatible} {holes}"
    );
}

/// `Wrap[T] = { inner: T }`, and an opaque `Opaque[T]: data`.
fn world(t: &TypeInterner) -> Vec<SignatureRef> {
    let p = t.intern(Type::with_kind(TypeKind::Parameter(
        TypeParameterRef::bound(TypeParamId(0)),
    )));
    let inner = t.intern(Type::with_kind(TypeKind::Record(
        vec![Field {
            name: "inner".into(),
            ty: p,
            growable: false,
        }]
        .into(),
    )));
    let mut wrap = (*nominal(Some(inner), vec![])).clone();
    wrap.definition = def("Wrap");
    let mut opaque = (*nominal(None, vec![Bound::Data])).clone();
    opaque.definition = def("Opaque");
    opaque.metadata.visibility = hirsel_syntax::Visibility::PublicOpaque;
    vec![Arc::new(wrap), Arc::new(opaque)]
}

/// A whole unique `(String)` taken into an aliased `(String)` field: the
/// payload view weakens the nested uniqueness, as main's walk does.
#[test]
fn a_taken_unique_part_is_received_through_its_payload_view() {
    let t = TypeInterner::new(None);
    let aliased = t.intern(Type::with_kind(TypeKind::Primitive(Primitive::String)));
    let mut unique = Type::with_kind(TypeKind::Primitive(Primitive::String));
    unique.modes.uniqueness = Uniqueness::Unique;
    let unique = t.intern(unique);
    let mut expected = Type::with_kind(TypeKind::Tuple(vec![aliased].into()));
    expected.modes.sendability = Sendability::NotSendable;
    let expected = t.intern(expected);
    let mut actual = Type::with_kind(TypeKind::Tuple(vec![unique].into()));
    actual.modes.uniqueness = Uniqueness::Unique;
    actual.modes.sendability = Sendability::Sendable;
    let actual = t.intern(actual);
    let generics = GenericDeclarations::default();
    let verdict = compatible_view_type_in(
        &t,
        expected,
        actual,
        TypeRelation::Value,
        &generics,
        &mut |_| Ok(None),
    );
    assert_eq!(verdict, Ok(true));
}

#[test]
fn nested_uniqueness_ignores_locality_and_keeps_payload_boundaries() {
    let t = TypeInterner::new(None);
    let aliased = t.intern(Type::with_kind(TypeKind::Primitive(Primitive::String)));
    let mut local_unique = Type::with_kind(TypeKind::Primitive(Primitive::String));
    local_unique.modes.uniqueness = Uniqueness::Unique;
    local_unique.modes.relation = Relation::environment();
    let unique = t.intern(local_unique);
    let tuple = |part| t.intern(Type::with_kind(TypeKind::Tuple(vec![part].into())));
    let mut declarations = world(&t);
    let SignatureKind::Type(wrap) = &mut Arc::make_mut(&mut declarations[0]).kind else {
        panic!("Wrap is a nominal type");
    };
    wrap.resource = true;
    let generics = GenericDeclarations::default();
    let mut lookup = |id| Ok(declarations.iter().find(|s| s.definition == id).cloned());
    let mut memo = ContextMemo::new(&t, &generics, &mut lookup);
    assert_eq!(
        memo.nested_uniqueness(tuple(aliased), tuple(unique), true),
        Ok(true)
    );
    assert_eq!(
        memo.nested_uniqueness(tuple(aliased), tuple(unique), false),
        Ok(false)
    );
    assert_eq!(
        memo.nested_uniqueness(tuple(unique), tuple(aliased), true),
        Ok(false)
    );
    let nominal = |name, part| {
        t.intern(Type::with_kind(TypeKind::Named {
            definition: def(name),
            arguments: vec![GenericArgument::Type(part)].into(),
        }))
    };
    // Resource Wrap is non-Data; Opaque explicitly promises Data.
    assert_eq!(
        memo.nested_uniqueness(nominal("Wrap", aliased), nominal("Wrap", unique), true),
        Ok(false)
    );
    assert_eq!(
        memo.nested_uniqueness(nominal("Opaque", aliased), nominal("Opaque", unique), true),
        Ok(true)
    );
}
