//! Original-entry data controls, not V, CT5 specialization or receipt authority.
use super::*;
use crate::sealed_source::tests::{assign, body, witness_definition};
use hirsel_tir::{BodyId, LocalId, Storage};
use hirsel_types::*;
use std::sync::Arc;

fn function(parameters: &[(PassingMode, TypeId)], result: TypeId) -> FunctionType {
    FunctionType {
        state_promises: Default::default(),
        generics: Default::default(),
        parameters: parameters
            .iter()
            .map(|&(passing, ty)| Parameter {
                name: None,
                label: None,
                passing,
                ty,
            })
            .collect::<Vec<_>>()
            .into(),
        result,
        error: None,
        allocation: Allocation::MayAllocate,
        authority: AuthoritySummary::inert(),
        uses: Uses::Only(AuthoritySummary::inert()),
        contracts: Default::default(),
        incomplete: false,
        foreign: None,
    }
}
fn types() -> (Arc<TypeInterner>, TypeId, TypeId) {
    let types = TypeInterner::new(None);
    let scalar = types.intern(Type::with_kind(TypeKind::Primitive(Primitive::U64)));
    let bytes = types.intern(Type::with_kind(TypeKind::Primitive(Primitive::Bytes)));
    (types, scalar, bytes)
}
fn named() -> (Arc<TypeInterner>, SourceInput, SignatureRef, InstanceKey) {
    let (types, scalar, bytes) = types();
    let mut definition = witness_definition(body(&[bytes, bytes, bytes, scalar], 3));
    Arc::make_mut(&mut definition).signature_hash = ContentHash([3; 32]);
    let signature = Arc::new(Signature {
        metadata: Default::default(),
        definition: definition.path,
        kind: SignatureKind::Function(function(
            &[
                (PassingMode::Borrow, bytes),
                (PassingMode::Take, bytes),
                (PassingMode::Mut, bytes),
            ],
            scalar,
        )),
        fingerprint: definition.signature_hash,
    });
    let key = InstanceKey::new(definition.path.path(), 0, &[], &types, None, None).unwrap();
    let source = SourceInput::capture(ContentHash([1; 32]), ContentHash([2; 32]), definition);
    (types, source, signature, key)
}
fn closure(capture_types: &[bool]) -> (Arc<TypeInterner>, SourceInput, InstanceKey) {
    let (types, scalar, bytes) = types();
    let callable = types.intern(Type::with_kind(TypeKind::Function(function(
        &[(PassingMode::Borrow, scalar)],
        scalar,
    ))));
    let parent_types = capture_types
        .iter()
        .map(|&byte| if byte { bytes } else { scalar })
        .chain([callable])
        .collect::<Vec<_>>();
    let mut parent = body(&parent_types, capture_types.len());
    let usages = [UseKind::Borrow, UseKind::Copy, UseKind::Take, UseKind::Mut];
    assign(
        &mut parent,
        capture_types.len() as u32,
        Rvalue::Closure {
            context: CaptureContext::Scoped,
            body: BodyId(1),
            captures: capture_types
                .iter()
                .enumerate()
                .map(|(ordinal, _)| hirsel_tir::Operand {
                    place: hirsel_tir::PlaceId(ordinal as u32),
                    usage: usages[ordinal % 4],
                })
                .collect(),
            destination: Destination::Heap,
        },
    );
    let child_types = std::iter::once(scalar)
        .chain(
            capture_types
                .iter()
                .map(|&byte| if byte { bytes } else { scalar }),
        )
        .collect::<Vec<_>>();
    let mut child = body(&child_types, 1);
    child.result = scalar;
    for local in child.locals.iter_mut().skip(1) {
        local.storage = Storage::Capture;
    }
    let mut definition = witness_definition(parent);
    Arc::make_mut(&mut definition).bodies.push(child);
    let key = InstanceKey::new(definition.path.path(), 1, &[], &types, None, None).unwrap();
    (
        types,
        SourceInput::capture(ContentHash([1; 32]), ContentHash([2; 32]), definition),
        key,
    )
}

#[test]
fn named_entry_keeps_original_modes_types_and_observation_binding() {
    let (types, source, signature, key) = named();
    let entry = source.named_entry(&key, &types, &signature, Ok).unwrap();
    assert_eq!(
        entry
            .parameters()
            .iter()
            .map(|p| (p.ordinal, p.local, p.passing))
            .collect::<Vec<_>>(),
        vec![
            (0, 0, Passing::Borrow),
            (1, 1, Passing::Take),
            (2, 2, Passing::Mut)
        ]
    );
    assert_eq!(entry.reference().instance, key.digest());
    assert_eq!(entry.source_identity(), ContentHash([1; 32]));
    assert_eq!(entry.implementation_revision(), ContentHash([2; 32]));
    assert_eq!(entry.parameters()[0].ty, entry.parameters()[2].ty);
    assert_ne!(entry.result(), &entry.parameters()[0].ty);
    assert!(entry.captures().is_empty());
    assert_eq!(
        entry.kind(),
        &SourceEntryKind::Definition {
            declaration: signature.fingerprint
        }
    );
}

#[test]
fn named_entry_rejects_unbound_declarations_and_mismatched_original_types() {
    let (types, source, signature, key) = named();
    for (case, expected) in [
        (0, "original entry declaration identity"),
        (1, "original entry declaration identity"),
        (2, "original entry parameter arity"),
        (3, "original entry parameter type differs"),
        (4, "original entry result type differs"),
    ] {
        let mut bad = (*signature).clone();
        match case {
            0 => {
                bad.definition =
                    hirsel_syntax::DefId::intern(&DefPath::core("another_source_entry"))
            }
            1 => bad.fingerprint = ContentHash([4; 32]),
            _ => {
                let SignatureKind::Function(function) = &mut bad.kind else {
                    unreachable!()
                };
                match case {
                    2 => function.parameters = Vec::new().into(),
                    3 => {
                        let mut parameters = function.parameters.to_vec();
                        parameters[0].ty = function.result;
                        function.parameters = parameters.into();
                    }
                    _ => function.result = function.parameters[0].ty,
                }
            }
        }
        assert_eq!(
            source.named_entry(&key, &types, &bad, Ok).unwrap_err(),
            SourceError::Malformed(expected)
        );
    }
    let mut repeated = source.clone();
    Arc::make_mut(&mut repeated.definition).bodies[0].parameters[1] = LocalId(0);
    assert_eq!(
        repeated
            .named_entry(&key, &types, &signature, Ok)
            .unwrap_err(),
        SourceError::Malformed("original entry parameter local repeated")
    );
}

#[test]
fn closure_entry_preserves_all_original_capture_ordinals_and_use_modes() {
    let (types, source, key) = closure(&[true, true, true, true]);
    let entry = source
        .closure_entry(&key, &types, BodyId(0), hirsel_tir::BlockId(0), 0, Ok)
        .unwrap();
    assert_eq!(
        entry
            .captures()
            .iter()
            .map(|c| (c.ordinal, c.local, c.place, c.usage))
            .collect::<Vec<_>>(),
        vec![
            (0, 1, 0, UseKind::Borrow),
            (1, 2, 1, UseKind::Copy),
            (2, 3, 2, UseKind::Take),
            (3, 4, 3, UseKind::Mut)
        ]
    );
    assert!(
        entry
            .captures()
            .iter()
            .all(|c| c.ty == entry.captures()[0].ty)
    );
    assert!(
        matches!(entry.kind(), SourceEntryKind::Closure { producer, context: CaptureContext::Scoped, .. }
        if producer.body == 0 && producer.block == 0 && producer.statement == 0)
    );
    assert_eq!(entry.parameters()[0].passing, Passing::Borrow);
}

#[test]
fn equal_callable_signatures_do_not_supply_capture_coverage() {
    let (types_a, source_a, key_a) = closure(&[true]);
    let (types_b, source_b, key_b) = closure(&[false]);
    let a = source_a
        .closure_entry(&key_a, &types_a, BodyId(0), hirsel_tir::BlockId(0), 0, Ok)
        .unwrap();
    let b = source_b
        .closure_entry(&key_b, &types_b, BodyId(0), hirsel_tir::BlockId(0), 0, Ok)
        .unwrap();
    let callable = |entry: &SourceCallableEntry| match entry.kind() {
        SourceEntryKind::Closure { callable, .. } => callable.clone(),
        _ => unreachable!(),
    };
    assert_eq!(callable(&a), callable(&b));
    assert_ne!(a.captures()[0].ty, b.captures()[0].ty);
}

#[test]
fn closure_entry_refuses_wrong_source_selection_missing_or_mismatched_captures() {
    let (types, source, key) = closure(&[true]);
    assert_eq!(
        source
            .closure_entry(&key, &types, BodyId(0), hirsel_tir::BlockId(0), 1, Ok)
            .unwrap_err(),
        SourceError::Malformed("original closure statement")
    );
    let wrong = InstanceKey::new(key.definition(), 0, &[], &types, None, None).unwrap();
    assert_eq!(
        source
            .closure_entry(&wrong, &types, BodyId(0), hirsel_tir::BlockId(0), 0, Ok)
            .unwrap_err(),
        SourceError::Malformed("original closure target body differs")
    );
    for missing in [true, false] {
        let mut bad = source.clone();
        let child = &mut Arc::make_mut(&mut bad.definition).bodies[1];
        if missing {
            child.locals[1].storage = Storage::Local;
        } else {
            child.locals[1].ty = child.locals[0].ty;
        }
        assert_eq!(
            bad.closure_entry(&key, &types, BodyId(0), hirsel_tir::BlockId(0), 0, Ok)
                .unwrap_err(),
            SourceError::Malformed(if missing {
                "original closure capture arity differs"
            } else {
                "original closure capture type differs"
            })
        );
    }
}

#[test]
fn generic_and_incomplete_entries_remain_unavailable_and_records_release_source() {
    let (types, source, signature, key) = named();
    let weak_definition = Arc::downgrade(&source.definition);
    let weak_signature = Arc::downgrade(&signature);
    let entry = source.named_entry(&key, &types, &signature, Ok).unwrap();
    for case in 0..3 {
        let mut bad = (*signature).clone();
        let SignatureKind::Function(function) = &mut bad.kind else {
            unreachable!()
        };
        let expected = match case {
            0 => {
                function.incomplete = true;
                "incomplete original callable entry"
            }
            1 => {
                function.generics = vec![GenericParameter::Size { name: "N".into() }].into();
                "original named entry needs complete binder selection"
            }
            _ => {
                function.foreign = Some(ForeignContract {
                    abi: "C".into(),
                    blocking: false,
                    aliasing_contract: "fixture".into(),
                });
                "original foreign entry needs reviewed adapter coverage"
            }
        };
        assert_eq!(
            source.named_entry(&key, &types, &bad, Ok).unwrap_err(),
            SourceError::Unsupported(expected)
        );
    }
    drop(source);
    drop(signature);
    assert!(weak_definition.upgrade().is_none());
    assert!(weak_signature.upgrade().is_none());
    assert_eq!(entry.parameters()[2].local, 2);
    let (types, mut source, key) = closure(&[true]);
    let destination = source.definition.bodies[0].places[1].ty;
    let mut row = types.get(destination).unwrap().clone();
    let TypeKind::Function(function) = &mut row.kind else {
        unreachable!()
    };
    function.generics = vec![GenericParameter::Size { name: "N".into() }].into();
    Arc::make_mut(&mut source.definition).bodies[0].places[1].ty = types.intern(row);
    assert_eq!(
        source
            .closure_entry(&key, &types, BodyId(0), hirsel_tir::BlockId(0), 0, Ok)
            .unwrap_err(),
        SourceError::Unsupported("original closure entry needs complete CT5 binder selection")
    );
}

#[test]
fn erased_original_callable_cannot_be_repaired_by_candidate_substitution() {
    let (types, mut source, key) = closure(&[true]);
    let callable = source.definition.bodies[0].places[1].ty;
    let erased = source.definition.bodies[0].places[0].ty;
    Arc::make_mut(&mut source.definition).bodies[0].places[1].ty = erased;
    let mut calls = 0;
    let error = source
        .closure_entry(&key, &types, BodyId(0), hirsel_tir::BlockId(0), 0, |_| {
            calls += 1;
            Ok(callable)
        })
        .unwrap_err();
    assert_eq!(
        error,
        SourceError::Unsupported("original closure destination lacks checked callable type")
    );
    assert_eq!(
        calls, 0,
        "unknown original type must refuse before substitution can invent evidence"
    );
}

fn generic_named(
    size: bool,
) -> (
    Arc<TypeInterner>,
    SourceInput,
    SignatureRef,
    crate::instance::Actuals,
    InstanceKey,
) {
    let (types, scalar, bytes) = types();
    let scope = body(&[scalar], 1).parameter_scope;
    let bound = if size {
        types.intern(Type::with_kind(TypeKind::Builtin {
            kind: BuiltinType::Array,
            arguments: vec![
                GenericArgument::Type(bytes),
                GenericArgument::Size(Size::Parameter(SizeParameterRef::bound(SizeParamId(0)))),
            ]
            .into(),
        }))
    } else {
        types.intern(Type::with_kind(TypeKind::Parameter(
            TypeParameterRef::bound(TypeParamId(0)),
        )))
    };
    let in_body = if size {
        types.intern(Type::with_kind(TypeKind::Builtin {
            kind: BuiltinType::Array,
            arguments: vec![
                GenericArgument::Type(bytes),
                GenericArgument::Size(Size::Parameter(SizeParameterRef::body(
                    scope,
                    SizeParamId(0),
                ))),
            ]
            .into(),
        }))
    } else {
        types.intern(Type::with_kind(TypeKind::Parameter(
            TypeParameterRef::body(scope, TypeParamId(0)),
        )))
    };
    let mut definition = witness_definition(body(&[in_body, in_body], 1));
    Arc::make_mut(&mut definition).signature_hash = ContentHash([3; 32]);
    let mut callable = function(&[(PassingMode::Borrow, bound)], bound);
    callable.generics = vec![if size {
        GenericParameter::Size { name: "N".into() }
    } else {
        GenericParameter::Type {
            name: "T".into(),
            bounds: Default::default(),
        }
    }]
    .into();
    let signature = Arc::new(Signature {
        metadata: Default::default(),
        definition: definition.path,
        kind: SignatureKind::Function(callable),
        fingerprint: definition.signature_hash,
    });
    let actuals = crate::instance::Actuals {
        binder: Some(scope),
        arguments: vec![if size {
            GenericArgument::Size(Size::Constant(3))
        } else {
            GenericArgument::Type(bytes)
        }],
        outer: vec![],
    };
    let key = crate::instance::key_of(
        &types,
        definition.path.path(),
        BodyId(0),
        &actuals,
        None,
        None,
    )
    .unwrap();
    (
        types,
        SourceInput::capture(ContentHash([1; 32]), ContentHash([2; 32]), definition),
        signature,
        actuals,
        key,
    )
}

#[test]
fn selected_named_entry_closes_both_original_type_binders() {
    let (types, source, signature, actuals, key) = generic_named(false);
    let entry = source
        .named_entry_selected(&key, &types, &signature, &actuals, &TypeMemo::new())
        .unwrap();
    let GenericArgument::Type(actual) = actuals.arguments[0] else {
        unreachable!()
    };
    let expected = crate::sealed_source::closed_storage_type(&types, actual).unwrap();
    assert_eq!(&entry.parameters()[0].ty, &expected);
    assert_eq!(entry.result(), &expected);
    assert_eq!(entry.parameters()[0].passing, Passing::Borrow);
    assert_eq!(entry.reference().instance, key.digest());
    // The legacy convenience method has no explicit selection and remains strict.
    assert_eq!(
        source
            .named_entry(&key, &types, &signature, Ok)
            .unwrap_err(),
        SourceError::Unsupported("original named entry needs complete binder selection")
    );
}

#[test]
fn selected_named_entry_closes_size_actuals_without_physical_layouts() {
    let (types, source, signature, actuals, key) = generic_named(true);
    let entry = source
        .named_entry_selected(&key, &types, &signature, &actuals, &TypeMemo::new())
        .unwrap();
    assert!(
        matches!(&entry.result().ty, hirsel_lir::operation::NativeType::Builtin(
        hirsel_lir::operation::BuiltinType::Array, args) if matches!(args.last(), Some(OperationArgument::Size(3))))
    );
    assert_eq!(entry.result(), &entry.parameters()[0].ty);
}

#[test]
fn selected_named_entry_refuses_wrong_key_scope_kind_and_incomplete_selection() {
    let (types, source, signature, actuals, key) = generic_named(false);
    for (case, expected) in [
        (0, "original named actual arity differs"),
        (1, "original named binder scope differs"),
        (2, "original named actual kind differs"),
        (3, "original named specialization key differs"),
    ] {
        let mut bad = actuals.clone();
        match case {
            0 => bad.arguments.clear(),
            1 => bad.binder = None,
            2 => bad.arguments[0] = GenericArgument::Size(Size::Constant(1)),
            _ => {
                bad.arguments[0] = GenericArgument::Type(
                    types.intern(Type::with_kind(TypeKind::Primitive(Primitive::U64))),
                )
            }
        }
        assert_eq!(
            source
                .named_entry_selected(&key, &types, &signature, &bad, &TypeMemo::new())
                .unwrap_err(),
            SourceError::Malformed(expected)
        );
    }
    let mut enclosing = actuals.clone();
    enclosing
        .outer
        .push((actuals.binder.unwrap(), actuals.arguments.clone()));
    assert_eq!(
        source
            .named_entry_selected(&key, &types, &signature, &enclosing, &TypeMemo::new())
            .unwrap_err(),
        SourceError::Unsupported("original named entry needs enclosing binder coverage")
    );
}

#[test]
fn selected_named_entry_preserves_source_modes_erased_by_base_instance_key() {
    let (types, source, signature, actuals, key) = generic_named(false);
    let ordinary = source
        .named_entry_selected(&key, &types, &signature, &actuals, &TypeMemo::new())
        .unwrap();
    let GenericArgument::Type(ty) = actuals.arguments[0] else {
        unreachable!()
    };
    let mut unique = types.get(ty).unwrap().clone();
    unique.modes.uniqueness = Uniqueness::Unique;
    let unique = types.intern(unique);
    let mut selected = actuals.clone();
    selected.arguments[0] = GenericArgument::Type(unique);
    let erased_key =
        crate::instance::key_of(&types, key.definition(), BodyId(0), &selected, None, None)
            .unwrap();
    assert_eq!(key, erased_key);
    let changed = source
        .named_entry_selected(&key, &types, &signature, &selected, &TypeMemo::new())
        .unwrap();
    assert_eq!(
        ordinary.parameters()[0].ty.ty,
        changed.parameters()[0].ty.ty
    );
    assert_ne!(
        ordinary.parameters()[0].ty.source_type,
        changed.parameters()[0].ty.source_type
    );
    assert_eq!(
        &changed.parameters()[0].ty,
        &crate::sealed_source::closed_storage_type(&types, unique).unwrap()
    );
}
