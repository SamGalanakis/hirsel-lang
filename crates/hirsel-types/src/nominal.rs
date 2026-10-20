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
/// Heights of the rows a proof reaches. Structural classes are the
/// interner's semantic normal forms: two valid graphs share a class exactly
/// when `semantic_eq` holds.
#[derive(Default)]
pub(crate) struct Shapes {
    heights: HashMap<TypeId, Option<usize>>,
}
impl Shapes {
    /// The class of a valid graph; None for one `semantic_eq` rejects.
    fn class(&self, t: &TypeInterner, root: TypeId) -> Option<TypeId> {
        t.valid(root).then(|| t.sem(root))
    }
    /// Rows on the longest edge path; None when the graph is invalid.
    fn height(&mut self, t: &TypeInterner, root: TypeId) -> Option<usize> {
        if let Some(height) = self.heights.get(&root) {
            return *height;
        }
        if !t.valid(root) {
            self.heights.insert(root, None);
            return None;
        }
        let mut pending = vec![(root, false)];
        while let Some((id, exit)) = pending.pop() {
            if self.heights.contains_key(&id) {
                continue;
            }
            let children: Vec<_> = t
                .row(id)
                .kind
                .graph_edges()
                .into_iter()
                .map(|e| e.ty)
                .collect();
            if !exit {
                pending.push((id, true));
                pending.extend(children.into_iter().map(|c| (c, false)));
                continue;
            }
            work::count(Counter::TypeNormalRows);
            let height = children
                .iter()
                .map(|c| self.heights[c].expect("a valid graph's rows have heights"))
                .max()
                .unwrap_or(0)
                + 1;
            self.heights.insert(id, Some(height));
        }
        self.heights[&root]
    }
    fn equal(&mut self, t: &TypeInterner, a: TypeId, b: TypeId) -> bool {
        let a = self.class(t, a);
        a.is_some() && a == self.class(t, b)
    }
    // Homeomorphic embedding, using a finite pair worklist instead of the host
    // stack. Some(true) includes a strict constructor insertion; None cannot embed.
    fn embedding(&mut self, t: &TypeInterner, old: TypeId, new: TypeId) -> Option<bool> {
        // A shrinking actual cannot embed the older argument. This also keeps
        // finite nested generic aliases out of the pair-product worklist.
        match (self.height(t, old), self.height(t, new)) {
            (Some(a), Some(b)) if a <= b => (),
            _ => return None,
        }
        let mut memo = std::collections::HashMap::new();
        let mut pending = vec![(old, new, false)];
        while let Some((a, b, exit)) = pending.pop() {
            if memo.contains_key(&(a, b)) {
                continue;
            }
            if self.equal(t, a, b) {
                memo.insert((a, b), Some(false));
                continue;
            }
            let (Some(ar), Some(br)) = (t.get(a), t.get(b)) else {
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
    fn arguments_grow(&mut self, t: &TypeInterner, old: TypeId, new: TypeId) -> bool {
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
        ) = (t.get(old), t.get(new))
        else {
            return false;
        };
        if a != b || aa.len() != ba.len() {
            return false;
        }
        // An older root taller than the new one has an argument taller than
        // every new argument, which cannot embed.
        match (self.height(t, old), self.height(t, new)) {
            (Some(a), Some(b)) if a <= b => (),
            _ => return false,
        }
        let mut grew = false;
        for (a, b) in aa.iter().zip(ba.iter()) {
            match (a, b) {
                (GenericArgument::Size(a), GenericArgument::Size(b)) if a == b => (),
                (GenericArgument::Type(a), GenericArgument::Type(b)) => {
                    match self.embedding(t, *a, *b) {
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
/// depends on it. Equality and growth read the session interner.
#[derive(Default)]
pub struct NominalExpansions {
    shapes: Shapes,
    memo: HashSet<(TypeId, usize)>,
    /// Entered roots, with the declaration whose `lowest` entry each pushed.
    active: Vec<(TypeId, usize, bool, Option<DefId>)>,
    /// Per declaration, the least height of its active instantiations at
    /// each nesting: only an instantiation no taller than a new root can
    /// embed in it, so a shrinking path is checked without a scan.
    lowest: HashMap<DefId, Vec<usize>>,
    growing: usize,
}
impl NominalExpansions {
    /// False assumes an already visited instantiation coinductively. The proof
    /// must still check every other owned component before publishing success.
    pub fn enter(
        &mut self,
        root: TypeId,
        context: usize,
        t: &TypeInterner,
    ) -> Result<bool, QueryError> {
        if let Some(class) = self.shapes.class(t, root)
            && !self.memo.insert((class, context))
        {
            return Ok(false);
        }
        let height = self.shapes.height(t, root);
        let definition = match t.get(root).map(|r| &r.kind) {
            Some(TypeKind::Named { definition, .. }) => Some(*definition),
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
                .any(|i| self.shapes.arguments_grow(t, self.active[i].0, root));
        if growth && self.growing == MAX_POLYMORPHIC_EXPANSIONS {
            return Err(type_proof_limit());
        }
        self.growing += usize::from(growth);
        let pushed = match (definition, height) {
            (Some(d), Some(h)) => {
                let lowest = self.lowest.entry(d).or_default();
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
        let store = TypeInterner::new(None);
        let mut actual = store.intern(row(TypeKind::Primitive(Primitive::U8)));
        let mut proof = NominalExpansions::default();
        for growing in 0..=MAX_POLYMORPHIC_EXPANSIONS + 1 {
            let root = store.intern(row(TypeKind::Named {
                definition: DefId::intern(&path("Wrap")),
                arguments: vec![GenericArgument::Type(actual)].into(),
            }));
            let result = proof.enter(root, 0, &store);
            if growing <= MAX_POLYMORPHIC_EXPANSIONS {
                assert_eq!(result, Ok(true));
            } else {
                assert_eq!(result, Err(type_proof_limit()));
            }
            actual = store.intern(row(TypeKind::Builtin {
                kind: BuiltinType::List,
                arguments: vec![GenericArgument::Type(actual)].into(),
            }));
        }
    }

    #[test]
    fn regular_recursive_properties_check_every_component_and_nonregular_recursion_is_operational()
    {
        for growing in [false, true] {
            let declaration = TypeInterner::new(None);
            let parameter = declaration.intern(row(TypeKind::Parameter(TypeParameterRef::bound(
                TypeParamId(0),
            ))));
            let actual = if growing {
                declaration.intern(row(TypeKind::Builtin {
                    kind: BuiltinType::List,
                    arguments: vec![GenericArgument::Type(parameter)].into(),
                }))
            } else {
                parameter
            };
            let next = declaration.intern(row(TypeKind::Named {
                definition: DefId::intern(&path("Wrap")),
                arguments: vec![GenericArgument::Type(actual)].into(),
            }));
            let representation = declaration.intern(row(TypeKind::Box(next)));
            let signature = nominal(Some(representation), vec![]);
            let store = std::sync::Arc::clone(&declaration);
            let scalar = store.intern(row(TypeKind::Primitive(Primitive::U8)));
            let root = store.intern(row(TypeKind::Named {
                definition: DefId::intern(&path("Wrap")),
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
        let store = TypeInterner::new(None);
        let scalar = store.intern(row(TypeKind::Primitive(Primitive::U8)));
        let boxed = store.intern(row(TypeKind::Box(scalar)));
        let old_actual = store.intern(row(TypeKind::Builtin {
            kind: BuiltinType::List,
            arguments: vec![GenericArgument::Type(scalar)].into(),
        }));
        let new_actual = store.intern(row(TypeKind::Builtin {
            kind: BuiltinType::List,
            arguments: vec![GenericArgument::Type(boxed)].into(),
        }));
        let old = store.intern(row(TypeKind::Named {
            definition: DefId::intern(&path("Wrap")),
            arguments: vec![GenericArgument::Type(old_actual)].into(),
        }));
        let new = store.intern(row(TypeKind::Named {
            definition: DefId::intern(&path("Wrap")),
            arguments: vec![GenericArgument::Type(new_actual)].into(),
        }));
        let mut shapes = Shapes::default();
        assert!(shapes.arguments_grow(&store, old, new));
        assert!(!shapes.arguments_grow(&store, new, old));
    }

    /// A small random graph over one store, with shared children, repeated
    /// rows, a non-edge mode difference, an erased-capture normalization case
    /// and recovery rows, so equal and unequal pairs both occur often.
    fn random_graph(seed: &mut u64, store: &TypeInterner, depth: u32) -> TypeId {
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
                definition: DefId::intern(&path(["Wrap", "Pair"][below(seed, 2) as usize])),
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
        store.intern(row)
    }

    #[test]
    fn structural_classes_agree_with_semantic_equality_on_random_graphs() {
        for case in 0..64u64 {
            let mut seed = 0x9e37_79b9_7f4a_7c15 ^ case;
            let left = TypeInterner::new(None);
            let right = std::sync::Arc::clone(&left);
            let a: Vec<_> = (0..8).map(|_| random_graph(&mut seed, &left, 3)).collect();
            let b: Vec<_> = (0..8).map(|_| random_graph(&mut seed, &right, 3)).collect();
            // Copies of one store's roots guarantee equal pairs; a graph
            // holding a recovery row cannot be imported.
            let copies: Vec<_> = a
                .iter()
                .filter_map(|t| Ok::<TypeId, GraphError>(*t).ok())
                .collect();
            let mut shapes = Shapes::default();
            let mut equal_pairs = 0;
            for x in &a {
                for y in b.iter().chain(&copies).chain(&a) {
                    let expected = semantic_equal(&left, *x, *y).unwrap_or(false);
                    assert_eq!(
                        shapes.equal(&left, *x, *y),
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
            let store = TypeInterner::new(None);
            let mut roots = vec![store.intern(row(TypeKind::Primitive(Primitive::U8)))];
            for _ in 0..depth {
                let inner = *roots.last().unwrap();
                roots.push(store.intern(row(TypeKind::Named {
                    definition: DefId::intern(&path("Wrap")),
                    arguments: vec![GenericArgument::Type(inner)].into(),
                })));
            }
            let mut proof = NominalExpansions::default();
            for root in roots.iter().rev() {
                assert_eq!(proof.enter(*root, 0, &store), Ok(true));
            }
            // Re-entering any instantiation is the coinductive assumption.
            assert_eq!(proof.enter(roots[depth / 2], 0, &store), Ok(false));
            proof.shapes.heights.len()
        };
        let (half, full) = (work(256), work(512));
        assert!(
            full <= 2 * half + 8,
            "{half} rows at depth 256, {full} at 512"
        );
    }
}
