//! `ContextMemo::prove` memoizes main's proof walk per row: every verdict,
//! dependency set and operational failure must equal main's fresh walk, for
//! every bound and flag combination, whatever earlier proofs the memo holds.
use crate::declarations::tests::{def, nominal, path, row};
use crate::properties::prove;
use crate::*;
use hirsel_syntax::{Bound, LexicalOwner};
use std::collections::HashMap;
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
}

fn parameter(t: &TypeInterner) -> TypeId {
    t.intern(row(TypeKind::Parameter(TypeParameterRef::bound(
        TypeParamId(0),
    ))))
}
fn named(t: &TypeInterner, name: &str, argument: TypeId) -> TypeId {
    t.intern(row(TypeKind::Named {
        definition: def(name),
        arguments: vec![GenericArgument::Type(argument)].into(),
    }))
}
fn declaration(name: &str, representation: Option<TypeId>, bounds: Vec<Bound>) -> SignatureRef {
    let mut signature = (*nominal(representation, bounds)).clone();
    signature.definition = def(name);
    if representation.is_none() {
        signature.metadata.visibility = hirsel_syntax::Visibility::PublicOpaque;
    }
    Arc::new(signature)
}

/// Recursive, budget-growing, mutually recursive, plain and opaque nominals.
fn declarations(t: &TypeInterner) -> HashMap<DefId, SignatureRef> {
    let p = parameter(t);
    let u8 = t.intern(row(TypeKind::Primitive(Primitive::U8)));
    let list = |a| {
        t.intern(row(TypeKind::Builtin {
            kind: BuiltinType::List,
            arguments: vec![GenericArgument::Type(a)].into(),
        }))
    };
    let boxed = |a| t.intern(row(TypeKind::Box(a)));
    let tuple = |parts: Vec<TypeId>| t.intern(row(TypeKind::Tuple(parts.into())));
    let chain = boxed(named(t, "Chain", p));
    let chain = t.intern(row(TypeKind::Variant(
        vec![
            Variant {
                name: "Nil".into(),
                fields: vec![].into(),
            },
            Variant {
                name: "Cons".into(),
                fields: vec![(None, p), (None, chain)].into(),
            },
        ]
        .into(),
    )));
    let grow = t.intern(row(TypeKind::Record(
        vec![Field {
            name: "inner".into(),
            ty: named(t, "Grow", list(p)),
            growable: false,
        }]
        .into(),
    )));
    let even = tuple(vec![p, named(t, "Odd", p)]);
    let odd = tuple(vec![boxed(named(t, "Even", p)), u8]);
    let pair = tuple(vec![p, p]);
    let fixed = tuple(vec![named(t, "Fixed", p), u8]);
    // A part that holds only by assuming its enclosing `Loop`, which then
    // fails on its own `T`: that part's verdict is no fact.
    let looped = tuple(vec![named(t, "Inner", p), p]);
    let inner = tuple(vec![named(t, "Loop", p), u8]);
    [
        declaration("Chain", Some(chain), vec![]),
        declaration("Grow", Some(grow), vec![]),
        declaration("Even", Some(even), vec![]),
        declaration("Odd", Some(odd), vec![]),
        declaration("Pair", Some(pair), vec![]),
        declaration("Fixed", Some(fixed), vec![]),
        declaration("Loop", Some(looped), vec![]),
        declaration("Inner", Some(inner), vec![]),
        declaration("Hidden", None, vec![Bound::Copyable]),
    ]
    .into_iter()
    .map(|s| (s.definition, s))
    .collect()
}

const NAMES: [&str; 9] = [
    "Chain", "Grow", "Even", "Odd", "Pair", "Fixed", "Loop", "Inner", "Hidden",
];

fn generate(r: &mut Random, t: &TypeInterner, depth: u32) -> TypeId {
    let choice = r.below(if depth == 0 { 4 } else { 11 });
    let child = |r: &mut Random| generate(r, t, depth - 1);
    let kind = match choice {
        0 => TypeKind::Primitive(Primitive::U8),
        1 => TypeKind::Primitive(Primitive::String),
        2 => TypeKind::Parameter(TypeParameterRef::bound(TypeParamId(r.below(2) as u32))),
        3 => TypeKind::Tuple(vec![].into()),
        4 | 5 => {
            let a = child(r);
            let b = if r.below(2) == 0 { a } else { child(r) };
            TypeKind::Tuple(vec![a, b].into())
        }
        6 => TypeKind::Box(child(r)),
        7 | 8 => TypeKind::Named {
            definition: def(NAMES[r.below(NAMES.len() as u64) as usize]),
            arguments: vec![GenericArgument::Type(child(r))].into(),
        },
        9 => {
            let kind =
                [BuiltinType::List, BuiltinType::Shared, BuiltinType::Inline][r.below(3) as usize];
            let mut arguments = vec![GenericArgument::Type(child(r))];
            if kind == BuiltinType::Inline {
                arguments.push(GenericArgument::Size(Size::Constant(2)));
            }
            TypeKind::Builtin {
                kind,
                arguments: arguments.into(),
            }
        }
        _ => {
            let result = child(r);
            TypeKind::Function(FunctionType {
                state_promises: Default::default(),
                generics: vec![].into(),
                parameters: vec![].into(),
                result,
                error: None,
                allocation: Allocation::NoAlloc,
                authority: AuthoritySummary::inert(),
                uses: Uses::Unrestricted,
                contracts: vec![].into(),
                incomplete: false,
                foreign: None,
            })
        }
    };
    let mut row = row(kind);
    if r.below(4) == 0 {
        row.modes.uniqueness = Uniqueness::Unique;
    }
    if r.below(6) == 0 {
        row.modes.multiplicity = Multiplicity::Once;
    }
    if r.below(5) == 0 {
        row.modes.relation = Relation::place(Origin {
            root: OriginRoot::Local(LexicalOwner::first(&path("f")), LocalId(r.below(2) as u32)),
            fields: vec![].into(),
        });
    }
    row.modes.sendability = match r.below(3) {
        0 => Sendability::Sendable,
        1 => Sendability::NotSendable,
        _ => Sendability::DependsOn(vec![TypeParameterRef::bound(TypeParamId(0))].into()),
    };
    t.intern(row)
}

fn every_flag() -> impl Iterator<Item = ProofFlags> {
    (0..16u8).map(|bits| ProofFlags {
        check_root_locality: bits & 1 != 0,
        use_evidence: bits & 2 != 0,
        shared_storage: bits & 4 != 0,
        handles: bits & 8 != 0,
    })
}

#[test]
fn memoized_proofs_agree_with_fresh_walks_across_shared_memos() {
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
    let (mut hits, mut checked) = (0, 0);
    for seed in 1..=24u64 {
        let mut r = Random(seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1);
        let t = TypeInterner::new(None);
        let world = declarations(&t);
        let mut roots: Vec<_> = (0..4).map(|_| generate(&mut r, &t, 4)).collect();
        // Every nominal at a small actual, so parts of one proof recur as
        // roots of others.
        let actuals = [
            t.intern(row(TypeKind::Primitive(Primitive::U8))),
            t.intern(row(TypeKind::Primitive(Primitive::String))),
            parameter(&t),
            generate(&mut r, &t, 1),
        ];
        for name in NAMES {
            let actual = actuals[r.below(actuals.len() as u64) as usize];
            roots.push(named(&t, name, actual));
        }
        let mut lookup = |d: DefId| Ok::<_, QueryError>(world.get(&d).cloned());
        let mut memo_lookup = |d: DefId| Ok::<_, QueryError>(world.get(&d).cloned());
        let mut memo = ContextMemo::new(&t, &generics, &mut memo_lookup);
        // Larger roots first, then their parts again: later proofs reuse
        // earlier interior entries in both directions.
        let order = roots.iter().chain(roots.iter().rev());
        for &root in order {
            for bound in [Bound::Plain, Bound::Copyable, Bound::Sendable, Bound::Flat] {
                for flags in every_flag() {
                    let fresh = prove(
                        &t,
                        root,
                        bound,
                        &generics,
                        flags.check_root_locality,
                        flags.use_evidence,
                        flags.shared_storage,
                        flags.handles,
                        &mut lookup,
                    );
                    let memoized = memo.prove(root, bound, flags);
                    assert_eq!(memoized, fresh, "seed {seed}: {root:?} {bound:?} {flags:?}");
                    checked += 1;
                }
            }
        }
        hits += memo.hits();
    }
    assert!(hits * 2 > checked, "{hits} hits in {checked} proofs");
}

#[test]
fn nested_instantiation_rows_prove_in_linear_work() {
    // `fn f(x: Wrap^k[Leaf]) -> Wrap^(k+1)[Leaf]`: modes proves every row of
    // the body, here every nesting. Each proof once walked the whole nest.
    let work = |depth: usize, outer_first: bool| {
        let t = TypeInterner::new(None);
        let p = parameter(&t);
        let inner = t.intern(row(TypeKind::Record(
            vec![Field {
                name: "inner".into(),
                ty: p,
                growable: false,
            }]
            .into(),
        )));
        let leaf = t.intern(row(TypeKind::Primitive(Primitive::U64)));
        let world: HashMap<DefId, SignatureRef> =
            [(def("Wrap"), declaration("Wrap", Some(inner), vec![]))].into();
        let mut rows = vec![leaf];
        for _ in 0..depth {
            rows.push(named(&t, "Wrap", *rows.last().unwrap()));
        }
        if outer_first {
            rows.reverse();
        }
        let mut lookups = 0;
        let mut lookup = |d: DefId| {
            lookups += 1;
            Ok::<_, QueryError>(world.get(&d).cloned())
        };
        let generics = GenericDeclarations::default();
        let mut memo = ContextMemo::new(&t, &generics, &mut lookup);
        for &row in &rows {
            for bound in [Bound::Plain, Bound::Copyable, Bound::Sendable, Bound::Flat] {
                assert_eq!(
                    memo.prove(row, bound, crate::properties::STRUCTURAL),
                    Ok(Sendability::Sendable)
                );
            }
        }
        drop(memo);
        lookups
    };
    for outer_first in [false, true] {
        let (half, full) = (work(128, outer_first), work(256, outer_first));
        assert!(
            full <= 2 * half + 16,
            "outer first {outer_first}: {half} lookups at depth 128, {full} at 256"
        );
    }
}

#[test]
fn representation_proof_keys_share_occurrences_without_losing_copy_or_crossing_modes() {
    let t = TypeInterner::new(None);
    let byte = t.intern(row(TypeKind::Primitive(Primitive::U8)));
    let mut first = row(TypeKind::Named {
        definition: def("Bits"),
        arguments: vec![GenericArgument::Type(byte)].into(),
    });
    first.modes.multiplicity = Multiplicity::Once;
    first.brand.alternatives = vec![Origin {
        root: OriginRoot::Local(LexicalOwner::first(&path("a")), LocalId(0)),
        fields: vec![FieldId(0)].into(),
    }]
    .into();
    let mut second = first.clone();
    second.modes.multiplicity = Multiplicity::Many;
    second.modes.relation = Relation::place(Origin {
        root: OriginRoot::Local(LexicalOwner::first(&path("b")), LocalId(1)),
        fields: vec![].into(),
    });
    second.brand.alternatives = vec![Origin {
        root: OriginRoot::Local(LexicalOwner::first(&path("b")), LocalId(1)),
        fields: vec![FieldId(2)].into(),
    }]
    .into();
    let (first, second) = (t.intern(first), t.intern(second));
    let declarations = GenericDeclarations::default();
    let signature = declaration("Bits", Some(byte), vec![Bound::Plain, Bound::Data]);
    let mut lookup = |d| Ok((d == def("Bits")).then(|| signature.clone()));
    let mut memo = ContextMemo::new(&t, &declarations, &mut lookup);
    assert!(memo.plain(first).unwrap());
    let before = memo.hits();
    assert!(memo.plain(second).unwrap());
    assert!(
        memo.hits() > before,
        "mode/brand-only occurrences must reuse the representation proof"
    );
    assert!(memo.data(first).unwrap());
    let before = memo.hits();
    assert!(memo.data(second).unwrap());
    assert!(memo.hits() > before);
    assert_eq!(
        memo.prove(first, Bound::Copyable, crate::properties::STRUCTURAL)
            .unwrap(),
        Sendability::NotSendable
    );
    assert_eq!(
        memo.prove(second, Bound::Copyable, crate::properties::STRUCTURAL)
            .unwrap(),
        Sendability::Sendable
    );
    assert_eq!(
        memo.prove(second, Bound::Sendable, crate::properties::STRUCTURAL)
            .unwrap(),
        Sendability::NotSendable
    );
    // Sharing representation keys must never turn an aliased counted value into
    // a unique crossing proof, or discharge another value's root locality.
    for unique in [false, true] {
        for scoped in [false, true] {
            let mut value = row(TypeKind::Primitive(Primitive::Bytes));
            value.modes.uniqueness = if unique {
                Uniqueness::Unique
            } else {
                Uniqueness::Aliased
            };
            if scoped {
                value.modes.relation = t.row(second).modes.relation.clone();
            }
            let value = t.intern(value);
            for bound in [Bound::Plain, Bound::Copyable, Bound::Sendable, Bound::Flat] {
                for flags in every_flag() {
                    let fresh = prove(
                        &t,
                        value,
                        bound,
                        &declarations,
                        flags.check_root_locality,
                        flags.use_evidence,
                        flags.shared_storage,
                        flags.handles,
                        &mut |_| Ok(None),
                    );
                    assert_eq!(memo.prove(value, bound, flags), fresh);
                }
            }
        }
    }
}

#[test]
fn leaf_proofs_use_local_rules_without_allocating_walk_frames() {
    let t = TypeInterner::new(None);
    let declarations = GenericDeclarations::signature(&[GenericParameter::Type {
        name: "T".into(),
        bounds: vec![].into(),
    }]);
    let byte = t.intern(row(TypeKind::Primitive(Primitive::U8)));
    let function = t.intern(row(TypeKind::Function(FunctionType {
        state_promises: Default::default(),
        generics: vec![].into(),
        parameters: vec![].into(),
        result: byte,
        error: None,
        allocation: Allocation::MayAllocate,
        authority: AuthoritySummary::Unknown,
        uses: Uses::Unrestricted,
        contracts: vec![].into(),
        incomplete: false,
        foreign: None,
    })));
    let mut roots = vec![
        byte,
        function,
        parameter(&t),
        t.intern(row(TypeKind::Error)),
    ];
    for primitive in [Primitive::String, Primitive::Bytes] {
        for unique in [false, true] {
            let mut row = row(TypeKind::Primitive(primitive));
            row.modes.uniqueness = if unique {
                Uniqueness::Unique
            } else {
                Uniqueness::Aliased
            };
            row.modes.multiplicity = Multiplicity::Once;
            row.modes.relation = Relation::place(Origin {
                root: OriginRoot::Local(LexicalOwner::first(&path("leaf")), LocalId(0)),
                fields: vec![].into(),
            });
            roots.push(t.intern(row));
        }
    }
    let before = crate::memo::proof_walk_rows();
    let mut lookup = |_| panic!("a leaf proof makes no signature lookup");
    let mut memo = ContextMemo::new(&t, &declarations, &mut lookup);
    for root in roots {
        for bound in [Bound::Plain, Bound::Copyable, Bound::Sendable, Bound::Flat] {
            for flags in every_flag() {
                let expected = prove(
                    &t,
                    root,
                    bound,
                    &declarations,
                    flags.check_root_locality,
                    flags.use_evidence,
                    flags.shared_storage,
                    flags.handles,
                    &mut |_| panic!("a leaf has no declaration"),
                );
                assert_eq!(memo.prove(root, bound, flags), expected);
                assert_eq!(memo.prove(root, bound, flags), expected);
            }
        }
    }
    assert_eq!(
        crate::memo::proof_walk_rows(),
        before,
        "local leaf judgments do not need a graph walk frame"
    );
    assert!(memo.hits() > 0, "leaf answers retain memo hits");
}
