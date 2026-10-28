//! The mode passes' proofs through one query memo agree with the free proofs
//! they replace, keep each bound, sharing state and declaration revision
//! apart, and keep main's directional compatibility in both orders.
use super::{
    content_sendability, copy_detaches, flat, property, sendability, sendability_after_sharing,
};
use hirsel_intern::Symbol;
use hirsel_syntax::{ContentHash, DefId, DefPath, ItemPath, PackageId};
use hirsel_types::*;
use std::cell::Cell;
use std::collections::HashMap;
use std::sync::Arc;

fn row(kind: TypeKind) -> Type {
    Type::with_kind(kind)
}
fn definition(name: &str) -> DefId {
    DefId::intern(&DefPath(ItemPath {
        package: PackageId {
            name: "modes_proofs".into(),
            version: "1".into(),
        },
        modules: vec![],
        name: name.into(),
    }))
}
fn nominal(name: &str, generics: usize, representation: TypeId) -> SignatureRef {
    Arc::new(Signature {
        definition: definition(name),
        metadata: SignatureMetadata::default(),
        fingerprint: ContentHash::of_bytes(name.as_bytes()),
        kind: SignatureKind::Type(Box::new(TypeDefinition {
            loop_source: None,
            format: None,
            destructor_allocation: Allocation::NoAlloc,
            generics: (0..generics)
                .map(|i| GenericParameter::Type {
                    name: format!("T{i}"),
                    bounds: vec![].into(),
                })
                .collect(),
            representation: Some(representation),
            bounds: vec![],
            authority: AuthoritySummary::inert(),
            capability: None,
            implementation_authority: None,
            serializable: false,
            serializable_check: None,
            layout: Layout::default(),
            distinct: false,
            resource: false,
            drop: None,
            fact: None,
            captures: None,
        })),
    })
}
fn record(t: &TypeInterner, fields: &[(&str, TypeId)]) -> TypeId {
    t.intern(row(TypeKind::Record(
        fields
            .iter()
            .map(|(name, ty)| Field {
                name: Symbol::intern(name),
                ty: *ty,
                growable: false,
            })
            .collect::<Vec<_>>()
            .into(),
    )))
}
fn named(t: &TypeInterner, name: &str, arguments: &[TypeId]) -> TypeId {
    t.intern(row(TypeKind::Named {
        definition: definition(name),
        arguments: arguments
            .iter()
            .map(|ty| GenericArgument::Type(*ty))
            .collect::<Vec<_>>()
            .into(),
    }))
}
fn builtin(t: &TypeInterner, kind: BuiltinType, argument: TypeId) -> TypeId {
    t.intern(row(TypeKind::Builtin {
        kind,
        arguments: vec![GenericArgument::Type(argument)].into(),
    }))
}
fn with_uniqueness(t: &TypeInterner, ty: TypeId, uniqueness: Uniqueness) -> TypeId {
    let mut r = t.row(ty).clone();
    r.modes.uniqueness = uniqueness;
    t.intern(r)
}

/// Declarations by definition, counting every read the provider sees.
struct Declarations {
    signatures: HashMap<DefId, SignatureRef>,
    reads: Cell<usize>,
}
impl Declarations {
    fn new(signatures: impl IntoIterator<Item = SignatureRef>) -> Self {
        Self {
            signatures: signatures.into_iter().map(|s| (s.definition, s)).collect(),
            reads: Cell::new(0),
        }
    }
    fn read(&self, d: DefId) -> Result<Option<SignatureRef>, QueryError> {
        self.reads.set(self.reads.get() + 1);
        Ok(self.signatures.get(&d).cloned())
    }
}

/// The rows under test: plain bits, owned heap content, a handle, a callable,
/// a nominal over each, unique and aliased.
fn rows(t: &TypeInterner) -> Vec<TypeId> {
    let scalar = t.intern(row(TypeKind::Primitive(Primitive::U64)));
    let list = builtin(t, BuiltinType::List, scalar);
    let shared = builtin(t, BuiltinType::Shared, scalar);
    let mutex = builtin(t, BuiltinType::Mutex, scalar);
    let unit = t.intern(row(TypeKind::Tuple(vec![].into())));
    let callable = t.intern(row(TypeKind::Function(FunctionType {
        state_promises: Default::default(),
        generics: vec![].into(),
        parameters: vec![].into(),
        result: unit,
        error: None,
        allocation: Allocation::MayAllocate,
        authority: AuthoritySummary::Unknown,
        uses: Uses::Unrestricted,
        contracts: vec![].into(),
        incomplete: false,
        foreign: None,
    })));
    let tuple = t.intern(row(TypeKind::Tuple(vec![scalar, list].into())));
    let leaf = named(t, "Leaf", &[]);
    let wrap_list = named(t, "Wrap", &[list]);
    let wrap_leaf = named(t, "Wrap", &[leaf]);
    let mut out = vec![];
    for ty in [
        scalar, list, shared, mutex, callable, tuple, leaf, wrap_list, wrap_leaf,
    ] {
        for uniqueness in [Uniqueness::Unique, Uniqueness::Aliased] {
            out.push(with_uniqueness(t, ty, uniqueness));
        }
    }
    out
}
fn declarations(t: &TypeInterner, leaf: TypeId) -> Declarations {
    let parameter = t.intern(row(TypeKind::Parameter(TypeParameterRef::bound(
        TypeParamId(0),
    ))));
    let wrap = record(t, &[("inner", parameter)]);
    let leaf = record(t, &[("v", leaf)]);
    Declarations::new([nominal("Leaf", 0, leaf), nominal("Wrap", 1, wrap)])
}

/// Every proof of the mode passes, through the memo and as the free function.
type Verdicts = Vec<String>;
fn memo_verdicts(memo: &mut ContextMemo<'_>, root: TypeId) -> Verdicts {
    let mut out = vec![];
    for p in [
        StructuralProperty::Plain,
        StructuralProperty::Copyable,
        StructuralProperty::NeedsRelease,
    ] {
        out.push(format!("{p:?} {:?}", property(memo, root, p)));
    }
    out.push(format!("flat {:?}", flat(memo, root)));
    out.push(format!("detaches {:?}", copy_detaches(memo, root)));
    out.push(format!("sendable {:?}", sendability(memo, root)));
    out.push(format!("content {:?}", content_sendability(memo, root)));
    out.push(format!(
        "shared {:?}",
        sendability_after_sharing(memo, root)
    ));
    out
}
fn free_verdicts(
    t: &TypeInterner,
    generics: &GenericDeclarations,
    declarations: &Declarations,
    root: TypeId,
) -> Verdicts {
    let mut lookup = |d| declarations.read(d);
    let mut out = vec![];
    for p in [
        StructuralProperty::Plain,
        StructuralProperty::Copyable,
        StructuralProperty::NeedsRelease,
    ] {
        out.push(format!(
            "{p:?} {:?}",
            structural_property(t, root, p, generics, &mut lookup)
        ));
    }
    out.push(format!(
        "flat {:?}",
        structural_flat(t, root, generics, &mut lookup)
    ));
    out.push(format!(
        "detaches {:?}",
        hirsel_types::copy_detaches(t, root, generics, &mut lookup)
    ));
    out.push(format!(
        "sendable {:?}",
        structural_sendability(t, root, generics, &mut lookup)
    ));
    out.push(format!(
        "content {:?}",
        hirsel_types::content_sendability(t, root, generics, &mut lookup)
    ));
    out.push(format!(
        "shared {:?}",
        hirsel_types::sendability_after_sharing(t, root, generics, &mut lookup)
    ));
    out
}

#[test]
fn one_memo_proves_each_row_under_each_bound_as_the_free_proofs_do() {
    let t = TypeInterner::new(None);
    let scalar = t.intern(row(TypeKind::Primitive(Primitive::U64)));
    let declarations = declarations(&t, scalar);
    let generics = GenericDeclarations::signature(&[]);
    let rows = rows(&t);
    let expected: Vec<_> = rows
        .iter()
        .map(|&root| free_verdicts(&t, &generics, &declarations, root))
        .collect();
    // A row's verdicts differ across bounds: the memo must not share them.
    assert!(
        expected
            .iter()
            .any(|v| v.iter().filter(|s| s.ends_with("Ok(true)")).count() > 0
                && v.iter().filter(|s| s.ends_with("Ok(false)")).count() > 0)
    );
    // One memo for every row, in both orders, twice: hits answer as misses.
    for reverse in [false, true] {
        let mut lookup = |d| declarations.read(d);
        let mut memo = ContextMemo::new(&t, &generics, &mut lookup);
        let mut order: Vec<_> = (0..rows.len()).collect();
        if reverse {
            order.reverse();
        }
        for _ in 0..2 {
            for &i in &order {
                assert_eq!(memo_verdicts(&mut memo, rows[i]), expected[i], "row {i}");
            }
        }
        assert!(memo.hits() > 0);
    }
}

#[test]
fn sharing_a_row_is_proved_apart_from_the_unique_row() {
    let t = TypeInterner::new(None);
    let scalar = t.intern(row(TypeKind::Primitive(Primitive::U64)));
    let declarations = declarations(&t, scalar);
    let generics = GenericDeclarations::signature(&[]);
    let mut differs = false;
    let mut lookup = |d| declarations.read(d);
    let mut memo = ContextMemo::new(&t, &generics, &mut lookup);
    for root in rows(&t) {
        let unique = with_uniqueness(&t, root, Uniqueness::Unique);
        let aliased = with_uniqueness(&t, root, Uniqueness::Aliased);
        // Before sharing: the unique row's crossing proof; after: the
        // aliased row's, with shared storage. Interleaved in one memo.
        let before = sendability(&mut memo, unique).unwrap();
        let after = sendability_after_sharing(&mut memo, aliased).unwrap();
        let mut free = |d| declarations.read(d);
        assert_eq!(
            before,
            structural_sendability(&t, unique, &generics, &mut free).unwrap()
        );
        assert_eq!(
            after,
            hirsel_types::sendability_after_sharing(&t, aliased, &generics, &mut free).unwrap()
        );
        assert_eq!(sendability(&mut memo, unique).unwrap(), before);
        differs |= before != after;
    }
    assert!(differs, "no row's crossing mode changes when it is shared");
}

#[test]
fn a_private_representation_edit_is_proved_by_the_next_query_and_hits_replay_reads() {
    let t = TypeInterner::new(None);
    let scalar = t.intern(row(TypeKind::Primitive(Primitive::U64)));
    let list = builtin(&t, BuiltinType::List, scalar);
    let generics = GenericDeclarations::signature(&[]);
    let root = named(&t, "Leaf", &[]);
    let mut verdicts = vec![];
    // The same row before and after Leaf's private representation changes
    // from plain bits to owned heap content: one memo per query revision.
    for field in [scalar, list] {
        let declarations = declarations(&t, field);
        let mut lookup = |d| declarations.read(d);
        let mut memo = ContextMemo::new(&t, &generics, &mut lookup);
        let plain = property(&mut memo, root, StructuralProperty::Plain).unwrap();
        let misses = declarations.reads.get();
        assert!(misses > 0);
        // A hit replays the declaration read into this query's provider.
        assert_eq!(
            property(&mut memo, root, StructuralProperty::Plain).unwrap(),
            plain
        );
        assert!(memo.hits() > 0);
        drop(memo);
        assert!(declarations.reads.get() > misses, "a hit made no read");
        let mut free = |d| declarations.read(d);
        assert_eq!(
            plain,
            structural_property(&t, root, StructuralProperty::Plain, &generics, &mut free).unwrap()
        );
        verdicts.push(plain);
    }
    assert_eq!(verdicts, [true, false]);
}

/// checker-fixes residue 1: proving every nesting of `Wrap^k[Leaf]`, as a
/// body's rows are classified, costs reads linear in `k`.
#[test]
fn proving_every_nesting_of_a_deep_row_is_linear() {
    let t = TypeInterner::new(None);
    let scalar = t.intern(row(TypeKind::Primitive(Primitive::U64)));
    let declarations = declarations(&t, scalar);
    let generics = GenericDeclarations::signature(&[]);
    let mut reads = vec![];
    for k in [64, 128, 256] {
        let mut nestings = vec![named(&t, "Leaf", &[])];
        for _ in 0..k {
            let inner = *nestings.last().unwrap();
            nestings.push(named(&t, "Wrap", &[inner]));
        }
        let before = declarations.reads.get();
        let mut lookup = |d| declarations.read(d);
        let mut memo = ContextMemo::new(&t, &generics, &mut lookup);
        for &root in nestings.iter().rev() {
            assert!(property(&mut memo, root, StructuralProperty::Plain).unwrap());
            assert!(property(&mut memo, root, StructuralProperty::Copyable).unwrap());
            assert_eq!(
                sendability_after_sharing(&mut memo, root).unwrap(),
                Sendability::Sendable
            );
        }
        drop(memo);
        reads.push(declarations.reads.get() - before);
    }
    assert!(
        reads[1] <= 2 * reads[0] + 64 && reads[2] <= 2 * reads[1] + 64,
        "reads per depth 64/128/256: {reads:?}"
    );
}

/// A callable returning a callable; the inner one's sendability and uses
/// are the nested promises under test.
fn producer(t: &TypeInterner, sendability: Sendability, uses: Uses) -> TypeId {
    let function = |result, uses: Uses| FunctionType {
        state_promises: Default::default(),
        generics: vec![].into(),
        parameters: vec![].into(),
        result,
        error: None,
        allocation: Allocation::MayAllocate,
        authority: match &uses {
            Uses::Only(authority) => authority.clone(),
            Uses::Unrestricted => AuthoritySummary::Unknown,
        },
        uses,
        contracts: vec![].into(),
        incomplete: false,
        foreign: None,
    };
    let unit = t.intern(row(TypeKind::Tuple(vec![].into())));
    let mut inner = row(TypeKind::Function(function(unit, uses)));
    inner.modes.sendability = sendability;
    let inner = t.intern(inner);
    t.intern(row(TypeKind::Function(function(inner, Uses::Unrestricted))))
}

/// Receiving (a call argument's view, a nested generic result's value view),
/// hand-back and the exact relation keep main's verdicts in both directions
/// when one query's memo answers them all, in either order.
#[test]
fn receiving_and_hand_back_keep_their_direction_in_one_memo() {
    let t = TypeInterner::new(None);
    let generics = GenericDeclarations::signature(&[]);
    let inert = || Uses::Only(AuthoritySummary::inert());
    let plain = producer(&t, Sendability::NotSendable, Uses::Unrestricted);
    let stronger = [
        producer(&t, Sendability::Sendable, Uses::Unrestricted),
        producer(&t, Sendability::NotSendable, inert()),
        producer(&t, Sendability::Sendable, inert()),
    ];
    for reverse in [false, true] {
        let mut lookup = |_: DefId| Ok::<Option<SignatureRef>, QueryError>(None);
        let mut memo = ContextMemo::new(&t, &generics, &mut lookup);
        let mut cases: Vec<_> = stronger.iter().copied().collect();
        if reverse {
            cases.reverse();
        }
        for _ in 0..2 {
            for &stronger in &cases {
                for relation in [TypeRelation::Value, TypeRelation::Parameter] {
                    // Variance: a nested result may promise more where it is
                    // received, never less.
                    assert!(memo.view(plain, stronger, relation).unwrap());
                    assert!(memo.compatible_type(plain, stronger, relation).unwrap());
                    assert!(!memo.view(stronger, plain, relation).unwrap());
                    assert!(!memo.compatible_type(stronger, plain, relation).unwrap());
                }
                // CLO-2: a `mut` argument is written back, so neither side
                // may promise more.
                for (expected, actual) in [(plain, stronger), (stronger, plain)] {
                    assert!(!memo.handback(expected, actual).unwrap());
                    assert!(
                        !memo
                            .compatible_type(expected, actual, TypeRelation::Exact)
                            .unwrap()
                    );
                    assert!(!memo.view(expected, actual, TypeRelation::Exact).unwrap());
                }
                assert!(memo.handback(stronger, stronger).unwrap());
            }
        }
    }
}

/// main's properties.rs:1528-1578 (MEM-5, `iter.from`): an iterator made
/// where its captures and cleanup are known is received where the written
/// `Iter[T]` states neither; the reverse forges evidence, a hand-back keeps
/// both exact, and a dependent capture promise is met only by itself.
#[test]
fn a_nested_erased_state_is_received_only_where_its_type_states_less() {
    let t = TypeInterner::new(None);
    let generics = GenericDeclarations::signature(&[]);
    let mut item = row(TypeKind::Primitive(Primitive::U64));
    item.drop_allocation = DropAllocation::NoAlloc;
    let item = t.intern(item);
    let iterator = |captures: Option<ErasedCaptures>, cleanup: DropAllocation| {
        let mut part = row(TypeKind::Builtin {
            kind: BuiltinType::Iter,
            arguments: vec![GenericArgument::Type(item)].into(),
        });
        part.erased_captures = captures;
        part.drop_allocation = cleanup;
        let part = t.intern(part);
        let holder = t.intern(row(TypeKind::Tuple(vec![part].into())));
        builtin(&t, BuiltinType::List, holder)
    };
    let known = || Some(ErasedCaptures::Known(AuthoritySummary::inert()));
    let written = iterator(None, DropAllocation::MayAllocate);
    let dependent = iterator(
        Some(ErasedCaptures::FromArguments(
            vec![ParameterRef::bound(ParamId(0)).into()].into(),
        )),
        DropAllocation::MayAllocate,
    );
    let made = [
        iterator(known(), DropAllocation::MayAllocate),
        iterator(None, DropAllocation::NoAlloc),
        iterator(known(), DropAllocation::NoAlloc),
    ];
    for reverse in [false, true] {
        let mut lookup = |_: DefId| Ok::<Option<SignatureRef>, QueryError>(None);
        let mut memo = ContextMemo::new(&t, &generics, &mut lookup);
        let mut cases = made.to_vec();
        if reverse {
            cases.reverse();
        }
        for &made in &cases {
            assert!(
                memo.compatible_type(written, made, TypeRelation::Value)
                    .unwrap()
            );
            assert!(
                !memo
                    .compatible_type(made, written, TypeRelation::Value)
                    .unwrap()
            );
            assert!(
                !memo
                    .compatible_type(dependent, made, TypeRelation::Value)
                    .unwrap()
            );
            assert!(
                memo.compatible_type(dependent, dependent, TypeRelation::Value)
                    .unwrap()
            );
            for (expected, actual) in [(written, made), (made, written)] {
                assert!(!memo.handback(expected, actual).unwrap());
            }
            assert!(memo.handback(made, made).unwrap());
        }
    }
}
