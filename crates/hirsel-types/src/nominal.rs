//! Declaration-path memoization for finite instantiated nominal graphs.
use crate::*;
use hirsel_intern::work::{self, Counter};
use std::collections::{HashMap, HashSet};

/// Operational budget for strictly growing polymorphic recursion. Ordinary
/// graph width, depth and regular recursive cycles do not consume this budget.
pub const MAX_POLYMORPHIC_EXPANSIONS: usize = 128;

pub fn type_proof_limit() -> QueryError {
    QueryError::Operational(OperationalFailure {
        operation: "type-proof-tooling-limit".into(),
        message: format!(
            "non-regular polymorphic recursion exceeded {MAX_POLYMORPHIC_EXPANSIONS} growing declaration expansions"
        ),
    })
}

// A constructor header with type edges erased. Sizes and declaration identity
// remain exact; a generic argument permutation is not constructor growth.
fn head(mut kind: TypeKind, marker: TypeId) -> TypeKind {
    match &mut kind {
        TypeKind::Tuple(ts) => ts.iter_mut().for_each(|t| *t = marker),
        TypeKind::Record(fs) => fs.iter_mut().for_each(|f| f.ty = marker),
        TypeKind::Variant(vs) => vs
            .iter_mut()
            .for_each(|v| v.fields.iter_mut().for_each(|f| f.1 = marker)),
        TypeKind::Box(t) => *t = marker,
        TypeKind::Named { arguments, .. } | TypeKind::Builtin { arguments, .. } => {
            arguments.iter_mut().for_each(|a| {
                if let GenericArgument::Type(t) = a {
                    *t = marker;
                }
            })
        }
        TypeKind::Function(f) => {
            f.parameters.iter_mut().for_each(|p| p.ty = marker);
            f.result = marker;
            if f.error.is_some() {
                f.error = Some(marker);
            }
        }
        _ => (),
    }
    kind
}
/// One row's structural class and height, memoized per TypeId. Rows are
/// immutable and every appended row has fresh provenance, so a TypeId names
/// one row for the process lifetime, whichever store a caller resolves it in.
#[derive(Clone, Copy, Debug)]
struct Shape {
    /// None for a graph `semantic_equal` rejects: one reaching a cycle, a
    /// missing row or a recovery row. Such a graph equals nothing.
    class: Option<u32>,
    /// Rows on the longest edge path; None when the graph reaches a cycle.
    height: Option<usize>,
}

/// Hash-consed structural classes: two valid graphs have one class exactly
/// when `semantic_equal` holds, since a class is the edge-erased row with
/// that function's normalization plus its children's classes in edge order.
/// Each row is shaped once, so equality and height cost one walk of the
/// rows reached, instead of a walk of both complete graphs per comparison.
pub(crate) struct Shapes {
    rows: HashMap<TypeId, Shape>,
    classes: HashMap<u64, Vec<(Type, Vec<u32>, u32)>>,
    count: u32,
    placeholder: TypeId,
}
impl Default for Shapes {
    fn default() -> Self {
        Self {
            rows: HashMap::new(),
            classes: HashMap::new(),
            count: 0,
            placeholder: TypeStore::fresh_id(0),
        }
    }
}
impl Shapes {
    fn shape<'s>(&mut self, root: TypeId, store: &impl Fn(TypeId) -> &'s TypeStore) -> Shape {
        if let Some(shape) = self.rows.get(&root) {
            return *shape;
        }
        let mut pending = vec![(root, false)];
        let mut path = HashSet::new();
        while let Some((t, exit)) = pending.pop() {
            if !exit && self.rows.contains_key(&t) {
                continue;
            }
            let row = store(t).get(t);
            let children: Vec<_> = row
                .map(|r| r.kind.graph_edges().into_iter().map(|e| e.ty).collect())
                .unwrap_or_default();
            if !exit {
                if !path.insert(t) {
                    // A back edge: `t` is on a cycle, so every row reaching it is too.
                    self.rows.insert(
                        t,
                        Shape {
                            class: None,
                            height: None,
                        },
                    );
                    continue;
                }
                pending.push((t, true));
                pending.extend(children.into_iter().map(|c| (c, false)));
                continue;
            }
            path.remove(&t);
            work::count(Counter::EqualityRows);
            let shapes: Vec<Shape> = children.iter().map(|c| self.rows[c]).collect();
            let height = shapes
                .iter()
                .try_fold(0, |h, s| s.height.map(|c| h.max(c)))
                .map(|h| h + 1);
            let class = match (row, height) {
                (Some(row), Some(_)) if !matches!(row.kind, TypeKind::Error) => shapes
                    .iter()
                    .map(|s| s.class)
                    .collect::<Option<Vec<_>>>()
                    .map(|children| self.intern(row, children)),
                _ => None,
            };
            self.rows.insert(t, Shape { class, height });
        }
        self.rows[&root]
    }
    fn intern(&mut self, row: &Type, children: Vec<u32>) -> u32 {
        use std::hash::{Hash, Hasher};
        let row = crate::graph::without_edges(row, self.placeholder);
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        std::mem::discriminant(&row.kind).hash(&mut hasher);
        if let TypeKind::Named { definition, .. } = &row.kind {
            definition.hash(&mut hasher);
        }
        children.hash(&mut hasher);
        let bucket = self.classes.entry(hasher.finish()).or_default();
        if let Some((_, _, class)) = bucket.iter().find(|(r, c, _)| *c == children && *r == row) {
            return *class;
        }
        bucket.push((row, children, self.count));
        self.count += 1;
        self.count - 1
    }
    fn equal<'s>(
        &mut self,
        a: TypeId,
        b: TypeId,
        store: &impl Fn(TypeId) -> &'s TypeStore,
    ) -> bool {
        let a = self.shape(a, store).class;
        a.is_some() && a == self.shape(b, store).class
    }
    // Homeomorphic embedding, using a finite pair worklist instead of the host
    // stack. Some(true) includes a strict constructor insertion; None cannot embed.
    fn embedding<'s>(
        &mut self,
        old: TypeId,
        new: TypeId,
        store: &impl Fn(TypeId) -> &'s TypeStore,
    ) -> Option<bool> {
        // A shrinking actual cannot embed the older argument. This also keeps
        // finite nested generic aliases out of the pair-product worklist.
        match (self.shape(old, store).height, self.shape(new, store).height) {
            (Some(a), Some(b)) if a <= b => (),
            _ => return None,
        }
        let mut memo = std::collections::BTreeMap::new();
        let mut pending = vec![(old, new, false)];
        while let Some((a, b, exit)) = pending.pop() {
            if memo.contains_key(&(a, b)) {
                continue;
            }
            if self.equal(a, b, store) {
                memo.insert((a, b), Some(false));
                continue;
            }
            let (Some(ar), Some(br)) = (store(a).get(a), store(b).get(b)) else {
                memo.insert((a, b), None);
                continue;
            };
            let ac = ar.kind.graph_edges();
            let bc = br.kind.graph_edges();
            let coupled = head(ar.kind.clone(), old) == head(br.kind.clone(), old);
            if !exit {
                pending.push((a, b, true));
                pending.extend(bc.iter().map(|c| (a, c.ty, false)));
                if coupled {
                    pending.extend(ac.iter().zip(&bc).map(|(a, b)| (a.ty, b.ty, false)));
                }
                continue;
            }
            let children = coupled
                .then(|| {
                    ac.iter()
                        .zip(&bc)
                        .map(|(a, b)| memo[&(a.ty, b.ty)])
                        .collect::<Option<Vec<_>>>()
                })
                .flatten();
            let result = if let Some(children) = children {
                Some(children.into_iter().any(|grew| grew))
            } else if bc.iter().any(|child| memo[&(a, child.ty)].is_some()) {
                Some(true)
            } else {
                None
            };
            memo.insert((a, b), result);
        }
        memo[&(old, new)]
    }

    /// All old arguments embed in the new arguments of the same declaration, with
    /// at least one strict constructor insertion. This includes growth inside a
    /// fixed outer constructor. Sizes cannot grow (LANG-7). Shrinking finite actuals
    /// and regular cycles/permutations spend no polymorphic expansion budget.
    fn arguments_grow<'s>(
        &mut self,
        old: TypeId,
        new: TypeId,
        store: &impl Fn(TypeId) -> &'s TypeStore,
    ) -> bool {
        let (
            Some(Type {
                kind:
                    TypeKind::Named {
                        definition: a,
                        arguments: aa,
                    },
                ..
            }),
            Some(Type {
                kind:
                    TypeKind::Named {
                        definition: b,
                        arguments: ba,
                    },
                ..
            }),
        ) = (store(old).get(old), store(new).get(new))
        else {
            return false;
        };
        if a != b || aa.len() != ba.len() {
            return false;
        }
        // An older root taller than the new one has an argument taller than
        // every new argument, which cannot embed.
        match (self.shape(old, store).height, self.shape(new, store).height) {
            (Some(a), Some(b)) if a <= b => (),
            _ => return false,
        }
        let mut grew = false;
        for (a, b) in aa.iter().zip(ba.iter()) {
            match (a, b) {
                (GenericArgument::Size(a), GenericArgument::Size(b)) if a == b => (),
                (GenericArgument::Type(a), GenericArgument::Type(b)) => {
                    match self.embedding(*a, *b, store) {
                        Some(growth) => grew |= growth,
                        None => return false,
                    }
                }
                _ => return false,
            }
        }
        grew
    }
}

/// A proof's finite instantiation memo and active declaration path. Context
/// distinguishes inherited uniqueness or ownership state when the property
/// depends on it. Equality and growth inspect the caller's rooted scratch,
/// through `store`, which names the store holding each TypeId.
#[derive(Default)]
pub struct NominalExpansions {
    shapes: Shapes,
    memo: HashSet<(u32, usize)>,
    /// Entered roots, with the declaration whose `lowest` entry each pushed.
    active: Vec<(TypeId, usize, bool, Option<DefPath>)>,
    /// Per declaration, the least height of its active instantiations at
    /// each nesting: only an instantiation no taller than a new root can
    /// embed in it, so a shrinking path is checked without a scan.
    lowest: HashMap<DefPath, Vec<usize>>,
    growing: usize,
}
impl NominalExpansions {
    /// False assumes an already visited instantiation coinductively. The proof
    /// must still check every other owned component before publishing success.
    pub fn enter<'s>(
        &mut self,
        root: TypeId,
        context: usize,
        store: impl Fn(TypeId) -> &'s TypeStore,
    ) -> Result<bool, QueryError> {
        if let Some(class) = self.shapes.shape(root, &store).class
            && !self.memo.insert((class, context))
        {
            return Ok(false);
        }
        let height = self.shapes.shape(root, &store).height;
        let definition = match store(root).get(root).map(|r| &r.kind) {
            Some(TypeKind::Named { definition, .. }) => Some(definition.clone()),
            _ => None,
        };
        let candidates = match (&definition, height) {
            (Some(d), Some(h)) => self
                .lowest
                .get(d)
                .and_then(|l| l.last())
                .is_some_and(|lowest| *lowest <= h),
            _ => false,
        };
        let growth = candidates
            && (0..self.active.len())
                .rev()
                .any(|i| self.shapes.arguments_grow(self.active[i].0, root, &store));
        if growth && self.growing == MAX_POLYMORPHIC_EXPANSIONS {
            return Err(type_proof_limit());
        }
        self.growing += usize::from(growth);
        let pushed = match (definition, height) {
            (Some(d), Some(h)) => {
                let lowest = self.lowest.entry(d.clone()).or_default();
                lowest.push(lowest.last().map_or(h, |l| h.min(*l)));
                Some(d)
            }
            _ => None,
        };
        self.active.push((root, context, growth, pushed));
        Ok(true)
    }
    pub fn leave(&mut self, root: TypeId, context: usize) {
        let (t, c, growth, definition) =
            self.active.pop().expect("nominal proof exit without entry");
        assert_eq!((t, c), (root, context));
        self.growing -= usize::from(growth);
        if let Some(definition) = definition {
            self.lowest.get_mut(&definition).unwrap().pop();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::declarations::tests::{nominal, path, row};

    #[test]
    fn only_strict_active_argument_growth_spends_the_operational_budget() {
        let mut store = TypeStore::default();
        let mut actual = store.push(row(TypeKind::Primitive(Primitive::U8)));
        let mut proof = NominalExpansions::default();
        for growing in 0..=MAX_POLYMORPHIC_EXPANSIONS + 1 {
            let root = store.push(row(TypeKind::Named {
                definition: path("Wrap"),
                arguments: vec![GenericArgument::Type(actual)].into(),
            }));
            let result = proof.enter(root, 0, |_| &store);
            if growing <= MAX_POLYMORPHIC_EXPANSIONS {
                assert_eq!(result, Ok(true));
            } else {
                assert_eq!(result, Err(type_proof_limit()));
            }
            actual = store.push(row(TypeKind::Builtin {
                kind: BuiltinType::List,
                arguments: vec![GenericArgument::Type(actual)].into(),
            }));
        }
    }

    #[test]
    fn regular_recursive_properties_check_every_component_and_nonregular_recursion_is_operational()
    {
        for growing in [false, true] {
            let mut declaration = TypeStore::default();
            let parameter = declaration.push(row(TypeKind::Parameter(TypeParameterRef::bound(
                TypeParamId(0),
            ))));
            let actual = if growing {
                declaration.push(row(TypeKind::Builtin {
                    kind: BuiltinType::List,
                    arguments: vec![GenericArgument::Type(parameter)].into(),
                }))
            } else {
                parameter
            };
            let next = declaration.push(row(TypeKind::Named {
                definition: path("Wrap"),
                arguments: vec![GenericArgument::Type(actual)].into(),
            }));
            let representation = declaration.push(row(TypeKind::Box(next)));
            let signature = nominal(declaration, Some(representation), vec![]);
            let mut store = TypeStore::default();
            let scalar = store.push(row(TypeKind::Primitive(Primitive::U8)));
            let root = store.push(row(TypeKind::Named {
                definition: path("Wrap"),
                arguments: vec![GenericArgument::Type(scalar)].into(),
            }));
            let generics = GenericDeclarations::default();
            for bound in [Bound::Data, Bound::Key] {
                let result = structural_equality(&store, root, bound, &generics, &mut |_| {
                    Ok(Some(signature.clone()))
                });
                assert_eq!(
                    result,
                    if growing {
                        Err(type_proof_limit())
                    } else {
                        Ok(true)
                    }
                );
            }
            let copyable = structural_property(
                &store,
                root,
                StructuralProperty::Copyable,
                &generics,
                &mut |_| Ok(Some(signature.clone())),
            );
            assert_eq!(
                copyable,
                if growing {
                    Err(type_proof_limit())
                } else {
                    Ok(true)
                }
            );
            assert_eq!(
                structural_property(
                    &store,
                    root,
                    StructuralProperty::Plain,
                    &generics,
                    &mut |_| Ok(Some(signature.clone()))
                ),
                Ok(false)
            );
        }
    }

    #[test]
    fn growth_inside_a_fixed_constructor_is_detected_and_shrinking_is_not() {
        let mut store = TypeStore::default();
        let scalar = store.push(row(TypeKind::Primitive(Primitive::U8)));
        let boxed = store.push(row(TypeKind::Box(scalar)));
        let old_actual = store.push(row(TypeKind::Builtin {
            kind: BuiltinType::List,
            arguments: vec![GenericArgument::Type(scalar)].into(),
        }));
        let new_actual = store.push(row(TypeKind::Builtin {
            kind: BuiltinType::List,
            arguments: vec![GenericArgument::Type(boxed)].into(),
        }));
        let old = store.push(row(TypeKind::Named {
            definition: path("Wrap"),
            arguments: vec![GenericArgument::Type(old_actual)].into(),
        }));
        let new = store.push(row(TypeKind::Named {
            definition: path("Wrap"),
            arguments: vec![GenericArgument::Type(new_actual)].into(),
        }));
        let mut shapes = Shapes::default();
        assert!(shapes.arguments_grow(old, new, &|_| &store));
        assert!(!shapes.arguments_grow(new, old, &|_| &store));
    }

    /// A small random graph over one store, with shared children, repeated
    /// rows, a non-edge mode difference, an erased-capture normalization case
    /// and recovery rows, so equal and unequal pairs both occur often.
    fn random_graph(seed: &mut u64, store: &mut TypeStore, depth: u32) -> TypeId {
        let below = |seed: &mut u64, n: u64| {
            // xorshift64*, as in the property tests.
            *seed ^= *seed >> 12;
            *seed ^= *seed << 25;
            *seed ^= *seed >> 27;
            seed.wrapping_mul(0x2545_f491_4f6c_dd1d) % n
        };
        let choice = below(seed, if depth == 0 { 3 } else { 8 });
        let once = below(seed, 4) == 0;
        let capture = below(seed, 3);
        let kind = match choice {
            0 => TypeKind::Primitive([Primitive::U8, Primitive::Bool][below(seed, 2) as usize]),
            1 => TypeKind::Parameter(TypeParameterRef::bound(TypeParamId(below(seed, 2) as u32))),
            2 if below(seed, 8) == 0 => TypeKind::Error,
            2 => TypeKind::Tuple(vec![].into()),
            3 => TypeKind::Box(random_graph(seed, store, depth - 1)),
            4 | 5 => {
                let a = random_graph(seed, store, depth - 1);
                let b = if below(seed, 2) == 0 {
                    a
                } else {
                    random_graph(seed, store, depth - 1)
                };
                TypeKind::Tuple(vec![a, b].into())
            }
            6 => TypeKind::Named {
                definition: path(["Wrap", "Pair"][below(seed, 2) as usize]),
                arguments: vec![GenericArgument::Type(random_graph(seed, store, depth - 1))].into(),
            },
            _ => TypeKind::Builtin {
                kind: BuiltinType::List,
                arguments: vec![GenericArgument::Type(random_graph(seed, store, depth - 1))].into(),
            },
        };
        let mut row = row(kind);
        if once {
            row.modes.multiplicity = Multiplicity::Once;
        }
        // Unknown erased captures normalize to absent ones.
        row.erased_captures = match capture {
            0 => Some(ErasedCaptures::Known(AuthoritySummary::Unknown)),
            _ => None,
        };
        store.push(row)
    }

    #[test]
    fn structural_classes_agree_with_semantic_equality_on_random_graphs() {
        for case in 0..64u64 {
            let mut seed = 0x9e37_79b9_7f4a_7c15 ^ case;
            let mut left = TypeStore::default();
            let mut right = TypeStore::default();
            let a: Vec<_> = (0..8)
                .map(|_| random_graph(&mut seed, &mut left, 3))
                .collect();
            let b: Vec<_> = (0..8)
                .map(|_| random_graph(&mut seed, &mut right, 3))
                .collect();
            // Copies of one store's roots guarantee equal pairs; a graph
            // holding a recovery row cannot be imported.
            let copies: Vec<_> = a
                .iter()
                .filter_map(|t| import_type(&left, *t, &mut right).ok())
                .collect();
            let mut shapes = Shapes::default();
            let resolve = |id: TypeId| {
                if left.id(id.index()) == Some(id) {
                    &left
                } else {
                    &right
                }
            };
            let mut equal_pairs = 0;
            for x in &a {
                for y in b.iter().chain(&copies).chain(&a) {
                    let expected =
                        semantic_equal(resolve(*x), *x, resolve(*y), *y).unwrap_or(false);
                    assert_eq!(
                        shapes.equal(*x, *y, &resolve),
                        expected,
                        "case {case}: {x:?} {y:?}"
                    );
                    equal_pairs += usize::from(expected);
                }
            }
            assert!(equal_pairs > 0, "case {case} compares no equal pair");
        }
    }

    #[test]
    fn nested_nominal_proofs_shape_each_row_once() {
        // A proof entering Wrap[Wrap[...u8]] at every depth, as a property
        // proof does while expanding each representation, once compared
        // every entered instantiation with every earlier one and recomputed
        // heights for each active pair. Its work is now linear in depth.
        let work = |depth: usize| {
            let mut store = TypeStore::default();
            let mut roots = vec![store.push(row(TypeKind::Primitive(Primitive::U8)))];
            for _ in 0..depth {
                let inner = *roots.last().unwrap();
                roots.push(store.push(row(TypeKind::Named {
                    definition: path("Wrap"),
                    arguments: vec![GenericArgument::Type(inner)].into(),
                })));
            }
            let before = hirsel_intern::work::read(hirsel_intern::work::Counter::EqualityRows);
            let mut proof = NominalExpansions::default();
            for root in roots.iter().rev() {
                assert_eq!(proof.enter(*root, 0, |_| &store), Ok(true));
            }
            // Re-entering any instantiation is the coinductive assumption.
            assert_eq!(proof.enter(roots[depth / 2], 0, |_| &store), Ok(false));
            hirsel_intern::work::read(hirsel_intern::work::Counter::EqualityRows) - before
        };
        let (half, full) = (work(256), work(512));
        assert!(
            full <= 2 * half + 8,
            "{half} rows at depth 256, {full} at 512"
        );
    }
}
