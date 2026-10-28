//! The interner's done-when properties (compiler-arch §3): canonical
//! encodings, one id per row across promotion, memo context, generated actual
//! keys, and outputs independent of id assignment.
use crate::declarations::tests::{def, nominal, path, row};
use crate::*;
use hirsel_syntax::{Bound, LexicalOwner, PassingMode};
use std::sync::Arc;

fn primitive(t: &TypeInterner, p: Primitive) -> TypeId {
    t.intern(row(TypeKind::Primitive(p)))
}
fn callable(
    _: &TypeInterner,
    parameters: Vec<(&str, PassingMode, TypeId)>,
    result: TypeId,
) -> Type {
    row(TypeKind::Function(FunctionType {
        state_promises: Default::default(),
        generics: vec![].into(),
        parameters: parameters
            .into_iter()
            .map(|(name, passing, ty)| Parameter {
                name: Some(name.into()),
                label: None,
                passing,
                ty,
            })
            .collect::<Vec<_>>()
            .into(),
        result,
        error: None,
        allocation: Allocation::NoAlloc,
        authority: AuthoritySummary::inert(),
        uses: Uses::Unrestricted,
        contracts: vec![].into(),
        incomplete: false,
        foreign: None,
    }))
}
fn local(n: u32) -> Relation {
    Relation::place(Origin {
        root: OriginRoot::Local(LexicalOwner::first(&path("f")), LocalId(n)),
        fields: vec![].into(),
    })
}

/// The corpus roots: a nested callable with parameter names, a shared DAG, a
/// nominal with a size actual, a recovery row and a cycle. `reverse` builds
/// everything in the opposite order, after unrelated rows.
fn corpus(t: &TypeInterner, reverse: bool) -> Vec<TypeId> {
    if reverse {
        for p in [Primitive::Bool, Primitive::U64, Primitive::Bytes] {
            primitive(t, p);
        }
    }
    let builders: Vec<Box<dyn Fn(&TypeInterner) -> TypeId>> = vec![
        Box::new(|t| {
            let byte = primitive(t, Primitive::U8);
            let text = primitive(t, Primitive::String);
            let inner = t.intern(callable(t, vec![("x", PassingMode::Borrow, byte)], byte));
            t.intern(callable(
                t,
                vec![
                    ("f", PassingMode::Take, inner),
                    ("y", PassingMode::Mut, text),
                ],
                byte,
            ))
        }),
        Box::new(|t| {
            let byte = primitive(t, Primitive::U8);
            let mut unique = row(TypeKind::Box(byte));
            unique.modes.uniqueness = Uniqueness::Unique;
            unique.modes.relation = local(1);
            let shared = t.intern(unique);
            t.intern(row(TypeKind::Record(
                vec![
                    Field {
                        name: "a".into(),
                        ty: shared,
                        growable: false,
                    },
                    Field {
                        name: "b".into(),
                        ty: shared,
                        growable: true,
                    },
                ]
                .into(),
            )))
        }),
        Box::new(|t| {
            let text = primitive(t, Primitive::String);
            t.intern(row(TypeKind::Named {
                definition: def("Wrap"),
                arguments: vec![
                    GenericArgument::Type(text),
                    GenericArgument::Size(Size::Constant(3)),
                ]
                .into(),
            }))
        }),
        Box::new(|t| {
            let recovery = t.intern(row(TypeKind::Error));
            t.intern(row(TypeKind::Tuple(vec![recovery].into())))
        }),
        Box::new(|t| {
            let ids = t.intern_graph(2, |ids| {
                vec![
                    row(TypeKind::Box(ids[1])),
                    row(TypeKind::Tuple(vec![ids[0]].into())),
                ]
            });
            ids[0]
        }),
    ];
    let mut roots: Vec<TypeId> = if reverse {
        builders.iter().rev().map(|b| b(t)).collect()
    } else {
        builders.iter().map(|b| b(t)).collect()
    };
    if reverse {
        roots.reverse();
    }
    roots
}

fn signature(definition: &str, kind: SignatureKind) -> Signature {
    Signature {
        definition: def(definition),
        metadata: SignatureMetadata::default(),
        fingerprint: Default::default(),
        kind,
    }
}

fn function(t: &TypeInterner, root: TypeId) -> Signature {
    let unit = t.intern(Type::unit());
    let TypeKind::Function(f) =
        callable(t, vec![("callback", PassingMode::Borrow, root)], unit).kind
    else {
        unreachable!()
    };
    signature("uses", SignatureKind::Function(f))
}

#[test]
fn encodings_ignore_insertion_order_and_keep_names_and_invalid_graphs() {
    let (a, b) = (TypeInterner::new(None), TypeInterner::new(None));
    let (left, right) = (corpus(&a, false), corpus(&b, true));
    assert_ne!(
        left.iter().map(|id| format!("{id:?}")).collect::<Vec<_>>(),
        right.iter().map(|id| format!("{id:?}")).collect::<Vec<_>>(),
        "the two orders number rows differently"
    );
    let (table, positions) = encode_table(&a, &left);
    assert_eq!((table.clone(), positions.clone()), encode_table(&b, &right));
    for (x, y) in left.iter().zip(&right) {
        let (ex, ey) = (
            encoding::canonical_type_encoding(&a, *x),
            encoding::canonical_type_encoding(&b, *y),
        );
        assert_eq!(ex.is_ok(), ey.is_ok());
        if let (Ok(ex), Ok(ey)) = (ex, ey) {
            assert_eq!(ex, ey);
            assert_eq!(
                encoding::fingerprint(&a, &function(&a, *x), &[]),
                encoding::fingerprint(&b, &function(&b, *y), &[])
            );
        }
    }
    // Recovery rows and cycles encode in tables though not in signatures.
    assert!(encoding::canonical_type_encoding(&a, left[3]).is_err());
    assert!(encoding::canonical_type_encoding(&a, left[4]).is_err());
    // Decoding interns each row once: back into its interner it is exact,
    // and into a fresh one it re-encodes to the same table.
    assert_eq!(
        positions
            .iter()
            .map(|p| decode_table(&a, &table)[*p as usize])
            .take(4)
            .collect::<Vec<_>>(),
        left[..4]
    );
    let fresh = TypeInterner::new(None);
    let ids = decode_table(&fresh, &table);
    let roots: Vec<_> = positions.iter().map(|p| ids[*p as usize]).collect();
    assert_eq!(encode_table(&fresh, &roots), (table.clone(), positions));
    // Nested callable parameter names survive the table, and differ in it.
    let TypeKind::Function(outer) = &fresh.row(roots[0]).kind else {
        panic!()
    };
    let TypeKind::Function(inner) = &fresh.row(outer.parameters[0].ty).kind else {
        panic!()
    };
    assert_eq!(inner.parameters[0].name.as_deref(), Some("x"));
    let renamed = {
        let byte = primitive(&fresh, Primitive::U8);
        fresh.intern(callable(
            &fresh,
            vec![("z", PassingMode::Borrow, byte)],
            byte,
        ))
    };
    assert!(fresh.semantic_eq(renamed, outer.parameters[0].ty));
    assert_ne!(
        encode_table(&fresh, &[renamed]).0,
        encode_table(&fresh, &[outer.parameters[0].ty]).0
    );
    // The persisted form is the table and positions, never session ids.
    let json = serde_json::to_string(&table).unwrap();
    assert_eq!(serde_json::from_str::<TypeTable>(&json).unwrap(), table);
    let scoped = with_table_encoding(&a, &left, || serde_json::to_string(&left).unwrap());
    assert_eq!(
        scoped,
        with_table_encoding(&b, &right, || serde_json::to_string(&right).unwrap())
    );
    let decoded: Vec<TypeId> = with_table_decoding(&ids, || serde_json::from_str(&scoped).unwrap());
    assert_eq!(decoded, roots);
    assert!(
        serde_json::to_string(&left[0]).is_err(),
        "no scope, no session id"
    );
}

#[test]
fn a_local_row_and_a_later_staged_signature_row_share_one_id() {
    let t = TypeInterner::new(None);
    // A body interns X first.
    let text = primitive(&t, Primitive::String);
    let local_x = t.intern(row(TypeKind::Named {
        definition: def("Wrap"),
        arguments: vec![GenericArgument::Type(text)].into(),
    }));
    let local_sem = t.sem(local_x);
    assert!(t.valid(local_x));
    // A signature staged elsewhere names X; promotion decodes its table.
    let elsewhere = TypeInterner::new(None);
    let staged = {
        let text = primitive(&elsewhere, Primitive::String);
        let x = elsewhere.intern(row(TypeKind::Named {
            definition: def("Wrap"),
            arguments: vec![GenericArgument::Type(text)].into(),
        }));
        let list = elsewhere.intern(row(TypeKind::Builtin {
            kind: BuiltinType::List,
            arguments: vec![GenericArgument::Type(x)].into(),
        }));
        encode_table(&elsewhere, &[list, x])
    };
    let before = t.stats();
    let ids = decode_table(&t, &staged.0);
    let staged_x = ids[staged.1[1] as usize];
    assert_eq!(staged_x, local_x, "one row, one id");
    // Memos computed for the local row serve the staged one.
    assert_eq!(t.sem(staged_x), local_sem);
    assert!(t.valid(staged_x));
    let after = t.stats();
    assert_eq!(after.rows, before.rows + 1, "only the list row is new");
    assert_eq!(after.sem_computed, before.sem_computed);
    assert_eq!(after.valid_computed, before.valid_computed);
    // Over an immutable base, session rows equal to base rows get base ids.
    let (base, base_ids) = TypeBase::from_table(&staged.0);
    let session = TypeInterner::new(Some(base));
    let ids = decode_table(&session, &staged.0);
    assert_eq!(ids, base_ids);
    let session_rows = session.stats().rows;
    assert_eq!(session_rows, 0);
    assert_eq!(decode_table(&session, &staged.0), ids);
    let text = primitive(&session, Primitive::String);
    let again = session.intern(row(TypeKind::Named {
        definition: def("Wrap"),
        arguments: vec![GenericArgument::Type(text)].into(),
    }));
    assert_eq!(again, ids[staged.1[1] as usize]);
    assert_eq!(session.stats().rows, session_rows);
}

#[test]
fn projection_depends_on_the_bound_and_the_private_view_it_is_proven_under() {
    let t = TypeInterner::new(None);
    let parameter = t.intern(row(TypeKind::Parameter(TypeParameterRef::bound(
        TypeParamId(0),
    ))));
    let hidden = t.intern(row(TypeKind::Named {
        definition: def("Hidden"),
        arguments: vec![].into(),
    }));
    let mut owner = row(TypeKind::Tuple(vec![parameter, hidden].into()));
    owner.modes.relation = local(2);
    let owner = t.intern(owner);
    let projected = |part: TypeId| {
        let mut row = t.row(part).clone();
        row.modes.relation = local(2);
        t.intern(row)
    };
    let shared = TypeMemo::new();
    let byte = primitive(&t, Primitive::U8);
    let mut visible = (*nominal(Some(byte), vec![])).clone();
    visible.definition = def("Hidden");
    let mut opaque = (*nominal(None, vec![])).clone();
    opaque.definition = def("Hidden");
    opaque.metadata.visibility = hirsel_syntax::Visibility::PublicOpaque;
    let (visible, opaque) = (Arc::new(visible), Arc::new(opaque));
    for plain_first in [true, false] {
        for bounds in if plain_first {
            [true, false]
        } else {
            [false, true]
        } {
            let generics = GenericDeclarations::signature(&[GenericParameter::Type {
                name: "T".into(),
                bounds: if bounds { vec![Bound::Plain] } else { vec![] }.into(),
            }]);
            for view in [&visible, &opaque] {
                let mut lookup = |_: DefId| Ok::<_, QueryError>(Some(Arc::clone(view)));
                let mut memo = ContextMemo::new(&t, &generics, &mut lookup);
                // A Plain part is a copy; any other part borrows the owner.
                let part = memo.projected(&shared, owner, parameter).unwrap();
                assert_eq!(
                    part,
                    if bounds {
                        parameter
                    } else {
                        projected(parameter)
                    }
                );
                let nominal = memo.projected(&shared, owner, hidden).unwrap();
                let plain = Arc::ptr_eq(view, &visible);
                assert_eq!(nominal, if plain { hidden } else { projected(hidden) });
            }
        }
    }
}

#[test]
fn data_and_view_policy_memos_replay_reads_and_stay_in_their_context() {
    let t = TypeInterner::new(None);
    let byte = primitive(&t, Primitive::U8);
    let hidden = t.intern(row(TypeKind::Named {
        definition: def("Hidden"),
        arguments: vec![].into(),
    }));
    let mut visible = (*nominal(Some(byte), vec![])).clone();
    visible.definition = def("Hidden");
    let mut opaque = visible.clone();
    opaque.metadata.visibility = hirsel_syntax::Visibility::PublicOpaque;
    for view in [visible, opaque] {
        let expected = view.metadata.visibility != hirsel_syntax::Visibility::PublicOpaque;
        let view = Arc::new(view);
        let generics = GenericDeclarations::default();
        let reads = std::cell::RefCell::new(vec![]);
        let mut lookup = |definition| {
            reads.borrow_mut().push(definition);
            Ok(Some(view.clone()))
        };
        let mut memo = ContextMemo::new(&t, &generics, &mut lookup);
        assert_eq!(memo.data(hidden).unwrap(), expected);
        assert_eq!(*reads.borrow(), [def("Hidden")]);
        reads.borrow_mut().clear();
        assert_eq!(memo.data(hidden).unwrap(), expected);
        assert_eq!(
            *reads.borrow(),
            [def("Hidden")],
            "a Data hit replays its read"
        );
        let policy = memo.view_policy(hidden).unwrap();
        assert_eq!(policy.forget_brands, expected);
        reads.borrow_mut().clear();
        assert_eq!(memo.view_policy(hidden).unwrap(), policy);
        assert_eq!(
            *reads.borrow(),
            [def("Hidden")],
            "a policy hit replays its read"
        );
        let set = memo.payload_set([hidden, byte, hidden]);
        assert_eq!(set, memo.payload_set([byte, hidden]));
    }
    let parameter = t.intern(row(TypeKind::Parameter(TypeParameterRef::bound(
        TypeParamId(0),
    ))));
    for bounds in [vec![Bound::Data], vec![]] {
        let expected = !bounds.is_empty();
        let generics = GenericDeclarations::signature(&[GenericParameter::Type {
            name: "T".into(),
            bounds: bounds.into(),
        }]);
        let mut lookup = |_| Ok(None);
        let mut memo = ContextMemo::new(&t, &generics, &mut lookup);
        assert_eq!(memo.data(parameter).unwrap(), expected);
        assert_eq!(memo.data(parameter).unwrap(), expected);
    }
}

#[test]
fn current_normalization_batch_matches_single_roots_across_binders_and_views() {
    let t = TypeInterner::new(None);
    let byte = primitive(&t, Primitive::U8);
    let parameter = t.intern(row(TypeKind::Parameter(TypeParameterRef::bound(
        TypeParamId(0),
    ))));
    let hidden = t.intern(row(TypeKind::Named {
        definition: def("Hidden"),
        arguments: vec![].into(),
    }));
    let pair = t.intern(row(TypeKind::Tuple(vec![parameter, hidden].into())));
    let callable = |plain| {
        let mut value = callable(&t, vec![], pair);
        let TypeKind::Function(function) = &mut value.kind else {
            unreachable!()
        };
        function.generics = vec![GenericParameter::Type {
            name: "T".into(),
            bounds: if plain { vec![Bound::Plain] } else { vec![] }.into(),
        }]
        .into();
        t.intern(value)
    };
    let (plain, unknown) = (callable(true), callable(false));
    let mut visible = (*nominal(Some(byte), vec![])).clone();
    visible.definition = def("Hidden");
    let mut opaque = visible.clone();
    opaque.metadata.visibility = hirsel_syntax::Visibility::PublicOpaque;
    for view in [visible, opaque] {
        let view = Arc::new(view);
        for bounds in [vec![], vec![Bound::Plain]] {
            let generics = GenericDeclarations::signature(&[GenericParameter::Type {
                name: "T".into(),
                bounds: bounds.into(),
            }]);
            for reverse in [false, true] {
                let mut roots = vec![
                    plain,
                    pair,
                    unknown,
                    hidden,
                    parameter,
                    pair,
                    TypeId::DANGLING,
                ];
                if reverse {
                    roots.reverse();
                }
                let mut single_reads = vec![];
                let expected: Vec<_> = roots
                    .iter()
                    .map(|root| {
                        canonical_current_type(&t, *root, &generics, &mut |d| {
                            single_reads.push(d);
                            Ok(Some(view.clone()))
                        })
                        .unwrap()
                    })
                    .collect();
                let mut batch_reads = vec![];
                canonical_current_types(&t, &mut roots, &generics, &mut |d| {
                    batch_reads.push(d);
                    Ok(Some(view.clone()))
                })
                .unwrap();
                assert_eq!(roots, expected);
                assert!(
                    batch_reads.len() < single_reads.len(),
                    "shared graph normalization must avoid repeated declaration work"
                );
                assert_eq!(
                    batch_reads
                        .into_iter()
                        .collect::<std::collections::BTreeSet<_>>(),
                    single_reads.into_iter().collect()
                );
            }
        }
    }
    assert!(matches!(
        canonical_current_types(
            &t,
            &mut [hidden],
            &GenericDeclarations::default(),
            &mut |_| Err(QueryError::NotImplemented)
        ),
        Err(QueryError::NotImplemented)
    ));
}

#[test]
fn authority_memos_replay_reads_and_bind_roles_generics_and_private_views() {
    let t = TypeInterner::new(None);
    let byte = primitive(&t, Primitive::U8);
    let hidden = t.intern(row(TypeKind::Named {
        definition: def("Hidden"),
        arguments: vec![].into(),
    }));
    let mut visible = (*nominal(Some(byte), vec![])).clone();
    visible.definition = def("Hidden");
    let SignatureKind::Type(declaration) = &mut visible.kind else {
        unreachable!()
    };
    declaration.authority = AuthoritySummary::inert();
    let heap = t.intern(Type {
        allocation_target: Some(AllocationTarget::Heap),
        ..t.row(hidden).clone()
    });
    let mut unknown = visible.clone();
    let SignatureKind::Type(declaration) = &mut unknown.kind else {
        unreachable!()
    };
    declaration.authority = AuthoritySummary::Unknown;
    for view in [visible, unknown] {
        let SignatureKind::Type(declaration) = &view.kind else {
            unreachable!()
        };
        let expected = declaration.authority.clone();
        let view = Arc::new(view);
        let reads = std::cell::RefCell::new(vec![]);
        let mut lookup = |definition| {
            reads.borrow_mut().push(definition);
            Ok(Some(view.clone()))
        };
        let generics = GenericDeclarations::default();
        let mut memo = ContextMemo::new(&t, &generics, &mut lookup);
        let roles = NominalRoles::default();
        assert_eq!(memo.authority(hidden, &roles).unwrap(), expected);
        reads.borrow_mut().clear();
        assert_eq!(memo.authority(hidden, &roles).unwrap(), expected);
        assert_eq!(*reads.borrow(), [def("Hidden")]);
        assert_eq!(memo.authority(heap, &roles).unwrap(), expected);
        assert_eq!(
            memo.authority(
                heap,
                &NominalRoles {
                    heap: Some(HeapRole {
                        arena: path("Hidden"),
                        operation: path("heap")
                    }),
                    ..roles
                }
            )
            .unwrap(),
            AuthoritySummary::inert()
        );
    }
    let parameter = t.intern(row(TypeKind::Parameter(TypeParameterRef::bound(
        TypeParamId(0),
    ))));
    for bounds in [vec![], vec![Bound::Inert]] {
        let generics = GenericDeclarations::signature(&[GenericParameter::Type {
            name: "T".into(),
            bounds: bounds.clone().into(),
        }]);
        let mut lookup = |_| Ok(None);
        let mut memo = ContextMemo::new(&t, &generics, &mut lookup);
        let expected = if bounds.is_empty() {
            AuthoritySummary::Known(
                vec![AuthorityTerm::TypeParameter(TypeParameterRef::bound(
                    TypeParamId(0),
                ))]
                .into(),
            )
        } else {
            AuthoritySummary::inert()
        };
        for _ in 0..2 {
            assert_eq!(
                memo.authority(parameter, &NominalRoles::default()).unwrap(),
                expected
            );
        }
    }
}

#[test]
fn instantiation_never_reuses_a_result_across_signature_or_private_shape_edits() {
    let t = TypeInterner::new(None);
    let memo = TypeMemo::new();
    let parameter = t.intern(row(TypeKind::Parameter(TypeParameterRef::bound(
        TypeParamId(0),
    ))));
    let pair = t.intern(row(TypeKind::Tuple(vec![parameter, parameter].into())));
    let byte = primitive(&t, Primitive::U8);
    let arguments = [GenericArgument::Type(byte)];
    let authorities = [AuthoritySummary::inert()];
    let substitution = Substitution {
        binder: Some(GenericBinder::Bound(0)),
        arguments: &arguments,
        authorities: &authorities,
        ..Default::default()
    };
    let result = |signature: &Signature| {
        let view = view_digest(&t, signature).unwrap();
        match instantiate(&t, &memo, signature, view, &substitution)
            .unwrap()
            .kind
        {
            SignatureKind::Function(f) => f.result,
            SignatureKind::Type(d) => d.representation.unwrap(),
            SignatureKind::Const { .. }
            | SignatureKind::Family(_)
            | SignatureKind::Implementation(_) => unreachable!(),
        }
    };
    // A resident edit of `f[T](x: T) -> T` to `-> (T, T)`.
    let header = |result| {
        let TypeKind::Function(f) =
            callable(&t, vec![("x", PassingMode::Borrow, parameter)], result).kind
        else {
            unreachable!()
        };
        signature("f", SignatureKind::Function(f))
    };
    let (before, after) = (header(parameter), header(pair));
    let is_byte = |id: TypeId| t.row(id).kind == TypeKind::Primitive(Primitive::U8);
    let is_pair = |id: TypeId| matches!(&t.row(id).kind, TypeKind::Tuple(parts) if parts.len() == 2 && parts.iter().all(|p| is_byte(*p)));
    let first = result(&before);
    assert!(is_byte(first));
    assert!(is_pair(result(&after)));
    assert_eq!(result(&before), first, "the first result is still served");
    // A private-shape edit behind an unchanged public fingerprint.
    let mut public = (*nominal(None, vec![])).clone();
    public.definition = def("Hidden");
    public.metadata.visibility = hirsel_syntax::Visibility::PublicOpaque;
    public.fingerprint = encoding::fingerprint(&t, &public, &[]).unwrap();
    let SignatureKind::Type(declaration) = &public.kind else {
        unreachable!()
    };
    let view = |representation| {
        let mut declaration = declaration.clone();
        declaration.representation = Some(representation);
        encoding::owner_view(&public, *declaration)
    };
    let (old, new) = (view(parameter), view(pair));
    assert_eq!(old.fingerprint, new.fingerprint);
    assert_ne!(
        view_digest(&t, &old).unwrap(),
        view_digest(&t, &new).unwrap()
    );
    assert!(is_byte(result(&old)));
    assert!(is_pair(result(&new)));
}

#[test]
fn generated_actual_keys_ignore_names_and_modes_and_spell_mains_instance_name() {
    let t = TypeInterner::new(None);
    let byte = primitive(&t, Primitive::U8);
    let named = |name| t.intern(callable(&t, vec![(name, PassingMode::Borrow, byte)], byte));
    let (x, y) = (named("x"), named("y"));
    assert_ne!(x, y);
    let key = |a: TypeId| generated_actual_key(&t, &[GenericArgument::Type(a)]);
    assert_eq!(key(x), key(y));
    let mut moded = t.row(x).clone();
    moded.modes.uniqueness = Uniqueness::Unique;
    moded.modes.relation = local(0);
    moded.modes.sendability = Sendability::NotSendable;
    let moded = t.intern(moded);
    assert_eq!(key(moded), key(x));
    let parameter = t.intern(row(TypeKind::Parameter(TypeParameterRef::bound(
        TypeParamId(0),
    ))));
    assert_eq!(key(parameter), None);
    assert_eq!(
        generated_actual_key(
            &t,
            &[GenericArgument::Size(Size::Parameter(
                SizeParameterRef::bound(SizeParamId(0))
            ))]
        ),
        None
    );
    // Main's instance name: the declaration and each actual's CT-3 closed
    // spelling, sizes in decimal, joined by ", ".
    let list = t.intern(row(TypeKind::Builtin {
        kind: BuiltinType::List,
        arguments: vec![GenericArgument::Type(y)].into(),
    }));
    let arguments = [
        GenericArgument::Type(list),
        GenericArgument::Size(Size::Constant(3)),
    ];
    let declaration = path("generate");
    let mains = vec![CanonicalSpelling::closed(&t, list), "3".to_owned()];
    let generated = generated_actual_key(&t, &arguments).unwrap();
    assert_eq!(generated.spellings(), mains);
    assert_eq!(
        generated.instance_name(&declaration),
        format!("{}[{}]", declaration, mains.join(", "))
    );
    // Keys order as main's spelling lists, and persist as them.
    let keys: Vec<_> = [vec![byte], vec![byte, byte], vec![list], vec![x]]
        .iter()
        .map(|a| {
            let a: Vec<_> = a.iter().map(|t| GenericArgument::Type(*t)).collect();
            generated_actual_key(&t, &a).unwrap()
        })
        .collect();
    let mut by_key = keys.clone();
    by_key.sort();
    let mut by_spelling = keys.clone();
    by_spelling.sort_by_key(|k| k.spellings());
    assert_eq!(by_key, by_spelling);
    let json = serde_json::to_string(&generated).unwrap();
    assert_eq!(
        serde_json::from_str::<GeneratedActualKey>(&json).unwrap(),
        generated
    );
}

/// Every id-independent output of the corpus, as text.
fn outputs() -> String {
    let t = TypeInterner::new(None);
    let roots = corpus(&t, false);
    let mut out = format!("{:?}\n", encode_table(&t, &roots));
    for root in &roots {
        out += &format!(
            "{:?} {:?}\n",
            encoding::canonical_type_encoding(&t, *root).ok(),
            encoding::fingerprint(&t, &function(&t, *root), &[]).ok()
        );
        let key = generated_actual_key(&t, &[GenericArgument::Type(*root)]);
        out += &format!("{:?}\n", key.map(|k| k.spellings()));
        for other in &roots {
            out += &format!("{} ", t.semantic_eq(*root, *other));
        }
        let generics = GenericDeclarations::default();
        let mut lookup = |_: DefId| Ok::<_, QueryError>(None);
        let mut memo = ContextMemo::new(&t, &generics, &mut lookup);
        for bound in [Bound::Plain, Bound::Copyable, Bound::Sendable] {
            out += &format!(
                "{:?} ",
                memo.prove(*root, bound, crate::properties::STRUCTURAL)
            );
        }
        out += "\n";
    }
    out
}

#[test]
fn type_outputs_do_not_depend_on_id_assignment() {
    let Some(runs) = hirsel_intern::under_salts(
        "arch_tests::type_outputs_do_not_depend_on_id_assignment",
        &[1, 2, 0x5eed],
        outputs,
    ) else {
        return;
    };
    assert!(runs.iter().all(|run| *run == runs[0]), "{runs:#?}");
    assert_eq!(
        runs[0],
        outputs(),
        "salted runs agree with the unsalted one"
    );
}

#[test]
fn concurrent_interning_gives_every_row_one_id() {
    // Eight threads intern overlapping 10k-row workloads (shared leaves,
    // per-thread and shared composites) as an -j8 session does.
    let t = TypeInterner::new(None);
    let work = |thread: u32| -> Vec<TypeId> {
        let mut out = vec![];
        for i in 0..10_000u32 {
            // Half the rows are shared by every thread, half are this thread's.
            let n = if i % 2 == 0 {
                i
            } else {
                100_000 + i * 8 + thread
            };
            let leaf = t.intern(row(TypeKind::Builtin {
                kind: BuiltinType::Inline,
                arguments: vec![
                    GenericArgument::Type(primitive(&t, Primitive::U8)),
                    GenericArgument::Size(Size::Constant(u64::from(n))),
                ]
                .into(),
            }));
            out.push(t.intern(row(TypeKind::Tuple(vec![leaf, leaf].into()))));
        }
        out
    };
    let runs: Vec<Vec<TypeId>> = std::thread::scope(|s| {
        let handles: Vec<_> = (0..8).map(|i| s.spawn(move || work(i))).collect();
        handles.into_iter().map(|h| h.join().unwrap()).collect()
    });
    for run in &runs[1..] {
        for i in (0..10_000).step_by(2) {
            assert_eq!(run[i], runs[0][i], "a shared row has one id");
        }
    }
    let stats = t.stats();
    // u8, then 5000 shared and 8 x 5000 private leaves, each with its tuple.
    assert_eq!(stats.rows, 1 + 2 * (5_000 + 8 * 5_000));
    eprintln!(
        "8 threads: {} rows, {} bytes, {} hits, {} us lock wait",
        stats.rows,
        stats.row_bytes,
        stats.hits,
        stats.lock_wait_ns / 1000
    );
}

#[test]
fn retained_nominal_roles_avoid_rehashing_without_merging_contexts() {
    let t = TypeInterner::new(None);
    let byte = primitive(&t, Primitive::U8);
    let hidden = t.intern(row(TypeKind::Named {
        definition: def("Hidden"),
        arguments: vec![].into(),
    }));
    let heap = t.intern(Type {
        allocation_target: Some(AllocationTarget::Heap),
        ..t.row(hidden).clone()
    });
    let mut signature = (*nominal(Some(byte), vec![])).clone();
    signature.definition = def("Hidden");
    let SignatureKind::Type(declaration) = &mut signature.kind else {
        unreachable!()
    };
    declaration.authority = AuthoritySummary::Unknown;
    let signature = Arc::new(signature);
    let roles = NominalRoles {
        heap: Some(HeapRole {
            arena: path("Hidden"),
            operation: path("heap"),
        }),
        ..NominalRoles::default()
    };
    let generics = GenericDeclarations::default();
    let reads = std::cell::RefCell::new(vec![]);
    let mut lookup = |definition| {
        reads.borrow_mut().push(definition);
        Ok(Some(signature.clone()))
    };
    let mut memo = ContextMemo::new(&t, &generics, &mut lookup).with_nominal_roles(&roles);
    for _ in 0..64 {
        reads.borrow_mut().clear();
        assert_eq!(
            memo.authority(hidden, &roles).unwrap(),
            AuthoritySummary::Unknown
        );
        assert_eq!(*reads.borrow(), [def("Hidden")]);
        reads.borrow_mut().clear();
        assert_eq!(
            memo.authority(heap, &roles).unwrap(),
            AuthoritySummary::inert()
        );
        assert!(reads.borrow().is_empty());
    }
    assert_eq!(
        memo.role_key_lookups, 1,
        "one content-key lookup binds the immutable context"
    );

    // Equal data at a different address retains the ordinary content key.
    let copied = roles.clone();
    let before = memo.hits();
    assert_eq!(
        memo.authority(heap, &copied).unwrap(),
        AuthoritySummary::inert()
    );
    assert_eq!(memo.hits(), before + 1);
    let missing = NominalRoles::default();
    assert_eq!(
        memo.authority(heap, &missing).unwrap(),
        AuthoritySummary::Unknown
    );
    // Public metadata passed through the unbound API may change at the same address.
    let mut other = roles.clone();
    assert_eq!(
        memo.authority(heap, &other).unwrap(),
        AuthoritySummary::inert()
    );
    other.heap = None;
    assert_eq!(
        memo.authority(heap, &other).unwrap(),
        AuthoritySummary::Unknown
    );
    assert_eq!(
        memo.authority(heap, &roles).unwrap(),
        AuthoritySummary::inert()
    );
    // A new query keeps its own provider/private view despite equal roles.
    let mut visible = (*signature).clone();
    let SignatureKind::Type(declaration) = &mut visible.kind else {
        unreachable!()
    };
    declaration.authority = AuthoritySummary::inert();
    let mut lookup = |_| Ok(Some(Arc::new(visible.clone())));
    let mut separate = ContextMemo::new(&t, &generics, &mut lookup).with_nominal_roles(&roles);
    assert_eq!(
        separate.authority(hidden, &roles).unwrap(),
        AuthoritySummary::inert()
    );
    assert_eq!(
        memo.authority(hidden, &roles).unwrap(),
        AuthoritySummary::Unknown
    );
}
