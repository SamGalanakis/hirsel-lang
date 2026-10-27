//! Quotient-graph and address DATA controls. Actual source/profile/target
//! completeness remains independent; these fixtures do not manufacture V proof.
use super::*;
use crate::sealed_source::contract::registry::{Context, ContextId, Expansion, Registry};
use hirsel_lir::sealed_payload_source::{ClosedStorageCase, ClosedStorageFunction};
use std::collections::BTreeSet;

fn register(registry: &mut Registry, fixture: &Fixture, types: &TypeInterner) -> ContextId {
    let reference = fixture.entry.reference();
    let base = InstanceKey::new(
        &reference.definition,
        reference.body,
        &[],
        types,
        None,
        None,
    )
    .unwrap();
    registry
        .register(Context {
            base: &base,
            entry: &fixture.entry,
            source: &fixture.projection,
            ports: &fixture.profiles,
        })
        .unwrap()
}

fn target_call(source: &mut Body, label: &str) {
    source.blocks.push(Block {
        instructions: vec![],
        terminator: Terminator::Unreachable,
        span: Span::default(),
    });
    call(source, 0, &[(0, UseKind::Mut), (1, UseKind::Mut)], Some(2));
    let Terminator::Call { callee, .. } = &mut source.blocks[0].terminator else {
        unreachable!()
    };
    *callee = Callee::Definition {
        path: DefPath::core(label),
        arguments: vec![],
    };
}

fn caller(
    types: &TypeInterner,
    bytes: TypeId,
    scalar: TypeId,
    label: &str,
    target: &str,
    demanded: u32,
) -> Fixture {
    let mut source = body(&[bytes, bytes, scalar], 2);
    target_call(&mut source, target);
    call(&mut source, 1, &[(demanded, UseKind::Borrow)], None);
    Fixture::named(
        types,
        source,
        &[PassingMode::Mut, PassingMode::Mut],
        label,
        2,
    )
}

fn link_caller(registry: &mut Registry, id: ContextId, target: ContextId) {
    registry
        .resolve(
            id,
            BTreeMap::from([
                (0, Expansion::Targets(vec![target])),
                (
                    1,
                    Expansion::External(ExternalDemand {
                        parameters: vec![0],
                        ..Default::default()
                    }),
                ),
            ]),
            BTreeMap::new(),
        )
        .unwrap();
}

#[test]
fn repeated_uses_share_one_context_before_disjoint_mut_demands_are_solved() {
    let (types, bytes, scalar, _) = types();
    let callee = Fixture::named(
        &types,
        body(&[bytes, bytes, scalar], 2),
        &[PassingMode::Mut, PassingMode::Mut],
        "context_callee",
        2,
    );
    let a = caller(
        &types,
        bytes,
        scalar,
        "context_caller_a",
        "context_callee",
        0,
    );
    let b = caller(
        &types,
        bytes,
        scalar,
        "context_caller_b",
        "context_callee",
        1,
    );
    let mut registry = Registry::default();
    let callee_id = register(&mut registry, &callee, &types);
    assert_eq!(callee_id, register(&mut registry, &callee, &types));
    let a = register(&mut registry, &a, &types);
    let b = register(&mut registry, &b, &types);
    assert_eq!(registry.len(), 3);
    registry
        .resolve(callee_id, BTreeMap::new(), BTreeMap::new())
        .unwrap();
    link_caller(&mut registry, a, callee_id);
    link_caller(&mut registry, b, callee_id);
    let requests = registry.solve().unwrap();
    let callee = &requests[callee_id.index()];
    assert_eq!(
        callee
            .declaration()
            .contract()
            .unwrap()
            .parameters
            .iter()
            .map(|p| (p.source, p.mode))
            .collect::<Vec<_>>(),
        [(0, ReceiptMode::MutSlot), (1, ReceiptMode::MutSlot)]
    );
    assert!(callee.aware_key().is_some());
    assert_eq!(requests.len(), 3);
}

#[test]
fn recursive_contexts_share_the_fixed_point_without_a_clone_cutoff() {
    let (types, bytes, scalar, _) = types();
    let a = caller(
        &types,
        bytes,
        scalar,
        "recursive_context_a",
        "recursive_context_b",
        0,
    );
    let b = caller(
        &types,
        bytes,
        scalar,
        "recursive_context_b",
        "recursive_context_a",
        1,
    );
    let mut registry = Registry::default();
    let a = register(&mut registry, &a, &types);
    let b = register(&mut registry, &b, &types);
    link_caller(&mut registry, a, b);
    link_caller(&mut registry, b, a);
    let requests = registry.solve().unwrap();
    for request in requests {
        assert_eq!(
            request
                .declaration()
                .contract()
                .unwrap()
                .parameters
                .iter()
                .map(|p| p.source)
                .collect::<Vec<_>>(),
            [0, 1]
        );
    }
}

#[test]
fn unknown_expansion_differs_from_explicitly_complete_empty_and_keeps_ordinary_key() {
    let (types, bytes, _, _) = types();
    let fixture = Fixture::named(
        &types,
        body(&[bytes], 1),
        &[PassingMode::Borrow],
        "context_empty",
        2,
    );
    let mut registry = Registry::default();
    assert!(registry.is_empty());
    let id = register(&mut registry, &fixture, &types);
    assert_eq!(
        registry.solve().unwrap_err(),
        SourceError::Unsupported("receipt context lacks complete original expansion")
    );
    registry
        .resolve(id, BTreeMap::new(), BTreeMap::new())
        .unwrap();
    let requests = registry.solve().unwrap();
    let request = &requests[id.index()];
    let base = InstanceKey::new(
        &fixture.entry.reference().definition,
        0,
        &[],
        &types,
        None,
        None,
    )
    .unwrap();
    assert_eq!(request.ordinary_key().encoding(), base.encoding());
    assert_eq!(request.ordinary_key().digest(), base.digest());
    assert!(request.aware_key().is_none());
    assert!(request.declaration().contract().is_none());
}

#[test]
fn demanded_request_uses_exact_profile_then_contract_namespaces() {
    let (types, bytes, scalar, _) = types();
    let mut source = body(&[bytes, scalar], 1);
    call(&mut source, 0, &[(0, UseKind::Borrow)], None);
    let fixture = Fixture::named(&types, source, &[PassingMode::Borrow], "context_address", 2);
    let mut registry = Registry::default();
    let id = register(&mut registry, &fixture, &types);
    registry
        .resolve(
            id,
            BTreeMap::from([(
                0,
                Expansion::External(ExternalDemand {
                    parameters: vec![0],
                    ..Default::default()
                }),
            )]),
            BTreeMap::new(),
        )
        .unwrap();
    let requests = registry.solve().unwrap();
    let request = &requests[id.index()];
    let context = request.ordinary_key().derived(
        "hirsel.closed-storage-profile.v1",
        &[&Context {
            base: request.ordinary_key(),
            entry: &fixture.entry,
            source: &fixture.projection,
            ports: &fixture.profiles,
        }
        .canonical_bytes()
        .unwrap()],
    );
    let aware = context.derived(
        "hirsel.receipt-demand.v1",
        &[&request
            .declaration()
            .contract()
            .unwrap()
            .canonical_bytes()
            .unwrap()],
    );
    assert_eq!(request.context_key(), &context);
    assert_eq!(request.aware_key(), Some(&aware));
    assert_ne!(request.aware_key().unwrap(), request.ordinary_key());
}

#[test]
fn full_source_profiles_stay_distinct_and_conflicting_collapse_is_rejected() {
    let (types, bytes, _, _) = types();
    let a = Fixture::named(
        &types,
        body(&[bytes], 1),
        &[PassingMode::Take],
        "context_modes",
        2,
    );
    let mut unique = types.get(bytes).unwrap().clone();
    unique.modes.uniqueness = Uniqueness::Unique;
    let unique = types.intern(unique);
    let b = Fixture::named(
        &types,
        body(&[unique], 1),
        &[PassingMode::Take],
        "context_modes",
        2,
    );
    let mut registry = Registry::default();
    let a_id = register(&mut registry, &a, &types);
    let b_id = register(&mut registry, &b, &types);
    assert_ne!(a_id, b_id);
    assert_eq!(registry.len(), 2);
    let base =
        InstanceKey::new(&b.entry.reference().definition, 0, &[], &types, None, None).unwrap();
    assert_eq!(
        registry
            .register(Context {
                base: &base,
                entry: &b.entry,
                source: &b.projection,
                ports: &a.profiles
            })
            .unwrap_err(),
        SourceError::Malformed("receipt profile differs from full original source type")
    );
    registry
        .resolve(a_id, BTreeMap::new(), BTreeMap::new())
        .unwrap();
    registry
        .resolve(b_id, BTreeMap::new(), BTreeMap::new())
        .unwrap();
    let requests = registry.solve().unwrap();
    assert_eq!(requests[0].ordinary_key(), requests[1].ordinary_key());
    assert_ne!(requests[0].context_key(), requests[1].context_key());
}

#[test]
fn incomplete_extra_or_conflicting_links_are_never_silently_replaced() {
    let (types, bytes, scalar, _) = types();
    let fixture = caller(&types, bytes, scalar, "context_links", "context_links", 0);
    let mut registry = Registry::default();
    let id = register(&mut registry, &fixture, &types);
    assert_eq!(
        registry
            .resolve(id, BTreeMap::new(), BTreeMap::new())
            .unwrap_err(),
        SourceError::Malformed("receipt expansions differ from original source selectors")
    );
    assert_eq!(
        registry
            .resolve(
                id,
                BTreeMap::from([
                    (0, Expansion::Targets(vec![])),
                    (1, Expansion::External(ExternalDemand::default()))
                ]),
                BTreeMap::new()
            )
            .unwrap_err(),
        SourceError::Unsupported("receipt call has no original targets")
    );
    link_caller(&mut registry, id, id);
    let before = registry.solve().unwrap()[0].aware_key().unwrap().clone();
    assert_eq!(
        registry
            .resolve(
                id,
                BTreeMap::from([
                    (0, Expansion::Targets(vec![id])),
                    (1, Expansion::External(ExternalDemand::default()))
                ]),
                BTreeMap::new()
            )
            .unwrap_err(),
        SourceError::Malformed("conflicting complete receipt target expansions")
    );
    assert_eq!(registry.solve().unwrap()[0].aware_key(), Some(&before));
}

#[test]
fn source_observation_edits_do_not_enter_the_semantic_request_key() {
    let (types, bytes, scalar, _) = types();
    let mut source = body(&[bytes, scalar], 1);
    call(&mut source, 0, &[(0, UseKind::Borrow)], None);
    let a = Fixture::named(
        &types,
        source.clone(),
        &[PassingMode::Borrow],
        "context_revisions",
        2,
    );
    let b = Fixture::named(
        &types,
        source,
        &[PassingMode::Borrow],
        "context_revisions",
        9,
    );
    let mut keys = Vec::new();
    for fixture in [&a, &b] {
        let mut registry = Registry::default();
        let id = register(&mut registry, fixture, &types);
        registry
            .resolve(
                id,
                BTreeMap::from([(
                    0,
                    Expansion::External(ExternalDemand {
                        parameters: vec![0],
                        ..Default::default()
                    }),
                )]),
                BTreeMap::new(),
            )
            .unwrap();
        let requests = registry.solve().unwrap();
        assert_eq!(
            requests[0].declaration().entry().implementation_revision(),
            fixture.entry.implementation_revision()
        );
        keys.push(requests[0].aware_key().unwrap().clone());
    }
    assert_eq!(keys[0], keys[1]);
    let mut registry = Registry::default();
    register(&mut registry, &a, &types);
    let base =
        InstanceKey::new(&b.entry.reference().definition, 0, &[], &types, None, None).unwrap();
    assert_eq!(
        registry
            .register(Context {
                base: &base,
                entry: &b.entry,
                source: &b.projection,
                ports: &b.profiles
            })
            .unwrap_err(),
        SourceError::Malformed("conflicting original data for receipt profile context")
    );
}

#[test]
fn internal_demand_with_no_carriers_keeps_an_aware_request() {
    let (types, bytes, scalar, _) = types();
    let mut source = body(&[scalar, bytes, scalar], 1);
    assign(&mut source, 1, Rvalue::Constant(Constant::Bytes(vec![7])));
    call(&mut source, 0, &[(1, UseKind::Borrow)], None);
    let fixture = Fixture::named(
        &types,
        source,
        &[PassingMode::Borrow],
        "context_internal",
        2,
    );
    let mut registry = Registry::default();
    let id = register(&mut registry, &fixture, &types);
    registry
        .resolve(
            id,
            BTreeMap::from([(
                0,
                Expansion::External(ExternalDemand {
                    parameters: vec![0],
                    ..Default::default()
                }),
            )]),
            BTreeMap::new(),
        )
        .unwrap();
    let requests = registry.solve().unwrap();
    let request = &requests[id.index()];
    assert!(request.declaration().contract().unwrap().is_empty());
    assert!(request.aware_key().is_some());
}

#[test]
fn a_derived_or_foreign_key_cannot_replace_the_original_lexical_base() {
    let (types, bytes, _, _) = types();
    let fixture = Fixture::named(
        &types,
        body(&[bytes], 1),
        &[PassingMode::Take],
        "context_original",
        2,
    );
    let base = InstanceKey::new(
        &fixture.entry.reference().definition,
        0,
        &[],
        &types,
        None,
        None,
    )
    .unwrap();
    let wrong = [
        base.derived("hirsel.receipt-demand.v1", &[b"already derived"]),
        InstanceKey::new(
            &DefPath::core("context_foreign"),
            0,
            &[],
            &types,
            None,
            None,
        )
        .unwrap(),
    ];
    let mut registry = Registry::default();
    for base in &wrong {
        assert_eq!(
            registry
                .register(Context {
                    base,
                    entry: &fixture.entry,
                    source: &fixture.projection,
                    ports: &fixture.profiles
                })
                .unwrap_err(),
            SourceError::Malformed("receipt context lacks its original lexical base")
        );
    }
    assert!(registry.is_empty());
}

#[test]
fn context_preserves_port_profiles_with_the_same_union_of_callable_cases() {
    let (types, _, scalar, _) = types();
    let callable = types.intern(Type::with_kind(TypeKind::Function(function(&[], scalar))));
    let mut a = Fixture::named(
        &types,
        body(&[callable, callable, scalar], 2),
        &[PassingMode::Borrow, PassingMode::Borrow],
        "context_ordered_ports",
        2,
    );
    let mut b = Fixture::named(
        &types,
        body(&[callable, callable, scalar], 2),
        &[PassingMode::Borrow, PassingMode::Borrow],
        "context_ordered_ports",
        2,
    );
    let code_type = crate::sealed_source::closed_storage_type(&types, callable).unwrap();
    let cases = ["code_a", "code_b"].map(|label| {
        let definition = DefPath::core(label);
        let base = InstanceKey::new(&definition, 0, &[], &types, None, None).unwrap();
        ClosedStorageCase {
            body: hirsel_lir::invocations::ReferenceBody {
                definition,
                body: 0,
                instance: base.digest(),
            },
            recipe: ContentHash::of_parts("synthetic-profile-data-case", [label.as_bytes()]),
            environment: None,
        }
    });
    let selected = |case: &ClosedStorageCase| ClosedStorageProfile {
        schema: ClosedStorageProfile::SCHEMA,
        root: code_type.clone(),
        functions: vec![ClosedStorageFunction {
            ty: code_type.clone(),
            cases: vec![case.clone()],
        }],
    };
    for (ports, order) in [(&mut a.profiles, [0, 1]), (&mut b.profiles, [1, 0])] {
        for (port, case) in order.into_iter().enumerate() {
            ports.insert(Port::Parameter(port as u32), selected(&cases[case]));
        }
    }
    let union = |fixture: &Fixture| {
        fixture
            .profiles
            .values()
            .flat_map(|profile| &profile.functions)
            .flat_map(|function| &function.cases)
            .map(|case| hirsel_codec::to_text(case))
            .collect::<BTreeSet<_>>()
    };
    assert_eq!(union(&a), union(&b));
    let mut registry = Registry::default();
    let left = register(&mut registry, &a, &types);
    let right = register(&mut registry, &b, &types);
    assert_ne!(left, right);
    for id in [left, right] {
        registry
            .resolve(id, BTreeMap::new(), BTreeMap::new())
            .unwrap();
    }
    let requests = registry.solve().unwrap();
    assert_eq!(
        requests[left.index()].ordinary_key(),
        requests[right.index()].ordinary_key()
    );
    assert_ne!(
        requests[left.index()].context_key(),
        requests[right.index()].context_key()
    );
}

#[test]
fn context_requires_every_original_port_even_without_boundary_demand() {
    let (types, bytes, scalar, _) = types();
    let mut fixture = Fixture::named(
        &types,
        body(&[bytes, scalar], 1),
        &[PassingMode::Borrow],
        "context_missing_port",
        2,
    );
    fixture.profiles.remove(&Port::Result);
    let base = InstanceKey::new(
        &fixture.entry.reference().definition,
        0,
        &[],
        &types,
        None,
        None,
    )
    .unwrap();
    assert_eq!(
        Registry::default()
            .register(Context {
                base: &base,
                entry: &fixture.entry,
                source: &fixture.projection,
                ports: &fixture.profiles
            })
            .unwrap_err(),
        SourceError::Malformed("receipt context needs a complete ordered port-profile map")
    );
}

#[test]
fn generated_links_keep_original_declaration_and_selected_body_distinct() {
    let (types, bytes, scalar, _) = types();
    let target = Fixture::named(
        &types,
        body(&[bytes, bytes, scalar], 2),
        &[PassingMode::Mut, PassingMode::Mut],
        "selected_generated_body",
        2,
    );
    let caller = caller(
        &types,
        bytes,
        scalar,
        "generated_data_caller",
        "generated_declaration",
        0,
    );
    let mut registry = Registry::default();
    let target_id = register(&mut registry, &target, &types);
    let caller_id = register(&mut registry, &caller, &types);
    let observed = crate::instance::SelectionRead {
        declaration: DefPath::core("generated_declaration"),
        arguments: Some(vec!["Bytes".into()]),
        generated: true,
        selected: Some(target.entry.reference().definition.clone()),
    };
    for mutation in 0..4 {
        let mut selection = observed.clone();
        match mutation {
            0 => selection.generated = false,
            1 => selection.arguments = None,
            2 => selection.declaration = DefPath::core("foreign_declaration"),
            3 => selection.selected = Some(DefPath::core("foreign_selected_body")),
            _ => unreachable!(),
        }
        assert_eq!(
            registry
                .resolve(
                    caller_id,
                    BTreeMap::from([
                        (
                            0,
                            Expansion::Generated {
                                selection,
                                target: target_id
                            }
                        ),
                        (1, Expansion::External(ExternalDemand::default())),
                    ]),
                    BTreeMap::new()
                )
                .unwrap_err(),
            SourceError::Malformed("receipt generated target differs from original selection")
        );
    }
    assert_eq!(
        registry
            .resolve(
                caller_id,
                BTreeMap::from([
                    (0, Expansion::Targets(vec![target_id])),
                    (1, Expansion::External(ExternalDemand::default())),
                ]),
                BTreeMap::new()
            )
            .unwrap_err(),
        SourceError::Malformed("receipt target differs from original definition")
    );
    registry
        .resolve(
            caller_id,
            BTreeMap::from([
                (
                    0,
                    Expansion::Generated {
                        selection: observed,
                        target: target_id,
                    },
                ),
                (1, Expansion::External(ExternalDemand::default())),
            ]),
            BTreeMap::new(),
        )
        .unwrap();
    registry
        .resolve(target_id, BTreeMap::new(), BTreeMap::new())
        .unwrap();
    assert_eq!(registry.solve().unwrap().len(), 2);
}

#[test]
fn arena_scope_expansions_refuse_wrong_original_identity_slot_and_grant_entry() {
    let (types, _, scalar, arena) = types();
    let callable = types.intern(Type::with_kind(TypeKind::Function(function(
        &[(PassingMode::Borrow, arena)],
        scalar,
    ))));
    let caller = |operation: &str| {
        let mut source = body(&[callable, scalar], 1);
        call(&mut source, 0, &[(0, UseKind::Borrow)], Some(1));
        if let Terminator::Call { callee, .. } = &mut source.blocks[0].terminator {
            *callee = Callee::Definition {
                path: DefPath::core(operation),
                arguments: vec![],
            };
        }
        Fixture::named(&types, source, &[PassingMode::Borrow], operation, 2)
    };
    let scope = caller("arena.with");
    let foreign = caller("user_arena_with");
    let good = Fixture::named(
        &types,
        body(&[arena, scalar], 1),
        &[PassingMode::Borrow],
        "ordinary_arena_callback",
        2,
    );
    let wrong_type = Fixture::named(
        &types,
        body(&[scalar, scalar], 1),
        &[PassingMode::Borrow],
        "wrong_callback_type",
        2,
    );
    let wrong_arity = Fixture::named(
        &types,
        body(&[arena, scalar, scalar], 2),
        &[PassingMode::Borrow; 2],
        "wrong_callback_arity",
        2,
    );
    let mut wrong_grant_body = body(&[arena, scalar], 1);
    wrong_grant_body.arena_scope_grant = Some(ArenaScopeGrant {
        intrinsic: DefPath::core("arena.bounded"),
        callback_parameter: ParamId(1),
        arena_parameter: ParamId(0),
    });
    let wrong_grant = Fixture::named(
        &types,
        wrong_grant_body,
        &[PassingMode::Borrow],
        "wrong_callback_grant",
        2,
    );
    let mut registry = Registry::default();
    let scope = register(&mut registry, &scope, &types);
    let foreign = register(&mut registry, &foreign, &types);
    let good = register(&mut registry, &good, &types);
    let wrong_type = register(&mut registry, &wrong_type, &types);
    let wrong_arity = register(&mut registry, &wrong_arity, &types);
    let wrong_grant = register(&mut registry, &wrong_grant, &types);
    for (caller, callback, targets) in [
        (foreign, 0, vec![good]),
        (scope, 1, vec![good]),
        (scope, 0, vec![]),
        (scope, 0, vec![wrong_type]),
        (scope, 0, vec![wrong_arity]),
        (scope, 0, vec![wrong_grant]),
    ] {
        assert!(
            registry
                .resolve(
                    caller,
                    BTreeMap::from([(0, Expansion::ArenaScope { callback, targets })]),
                    BTreeMap::new()
                )
                .is_err()
        );
    }
    registry
        .resolve(
            scope,
            BTreeMap::from([(
                0,
                Expansion::ArenaScope {
                    callback: 0,
                    targets: vec![good],
                },
            )]),
            BTreeMap::new(),
        )
        .unwrap();
}

#[test]
fn solved_requests_retain_complete_original_profiles_after_context_release() {
    let (types, bytes, scalar, _) = types();
    let mut fixture = Fixture::named(
        &types,
        body(&[bytes, scalar, scalar], 2),
        &[PassingMode::Borrow, PassingMode::Borrow],
        "retained_source_profiles",
        2,
    );
    let original = fixture.profiles.clone();
    let mut registry = Registry::default();
    let id = register(&mut registry, &fixture, &types);
    registry
        .resolve(id, BTreeMap::new(), BTreeMap::new())
        .unwrap();
    let requests = registry.solve().unwrap();
    drop(registry);
    fixture.profiles.clear();
    drop(fixture);
    let request = &requests[id.index()];
    // Even known-empty boundary demand retains the complete context. Losing
    // the undemanded scalar port would force a later C provider to infer it
    // from erased native data or reopen an already released source body.
    assert!(request.declaration().contract().is_none());
    assert_eq!(request.profiles(), &original);
    assert_ne!(
        request.profiles()[&Port::Parameter(0)].root,
        request.profiles()[&Port::Parameter(1)].root,
    );
    assert_eq!(
        request.profiles()[&Port::Parameter(1)].root,
        request.profiles()[&Port::Result].root,
    );
}

#[test]
fn receipt_source_binds_original_and_aware_identities_and_checks_complete_port_profiles() {
    use hirsel_lir::sealed_payload_source::{ReceiptPort, ReceiptSource};
    let (types, bytes, scalar, _) = types();
    let mut source = body(&[bytes, scalar, scalar], 2);
    call(
        &mut source,
        0,
        &[(0, UseKind::Borrow), (1, UseKind::Borrow)],
        None,
    );
    let fixture = Fixture::named(
        &types,
        source,
        &[PassingMode::Borrow, PassingMode::Borrow],
        "source_boundary_packet",
        2,
    );
    let mut registry = Registry::default();
    let id = register(&mut registry, &fixture, &types);
    registry
        .resolve(
            id,
            BTreeMap::from([(
                0,
                Expansion::External(ExternalDemand {
                    parameters: vec![0, 1],
                    ..Default::default()
                }),
            )]),
            BTreeMap::new(),
        )
        .unwrap();
    let requests = registry.solve().unwrap();
    let request = &requests[id.index()];
    let source = request.receipt_source().unwrap().unwrap();
    assert_eq!(&source.original, fixture.entry.reference());
    assert_eq!(source.instance, request.aware_key().unwrap().digest());
    assert_ne!(source.instance, source.original.instance);
    assert_eq!(source.source, fixture.entry.source_identity());
    assert_eq!(
        source.implementation_revision,
        fixture.entry.implementation_revision()
    );
    assert_eq!(
        source.profile(ReceiptPort::Result),
        fixture.profiles.get(&Port::Result)
    );
    assert_eq!(source.profiles.len(), fixture.profiles.len());
    let bytes = hirsel_codec::to_text(&source);
    let restored: ReceiptSource = hirsel_codec::from_text(&bytes).unwrap();
    assert_eq!(restored, source);
    restored.check_structure().unwrap();
    for mutation in 0..8 {
        let mut broken = source.clone();
        match mutation {
            0 => {
                broken.profiles.remove(0);
            }
            1 => broken.profiles.swap(0, 1),
            2 => broken.profiles[0].profile.root.source_type = ContentHash([9; 32]),
            3 => broken.source = ContentHash([0; 32]),
            4 => broken.instance = ContentHash([0; 32]),
            5 => broken.original.instance = ContentHash([0; 32]),
            6 => broken.contract.source_demand = ContentHash([9; 32]),
            7 => {
                let wrong = broken.profiles[1].profile.clone();
                broken.profiles[0].profile = wrong;
            }
            _ => unreachable!(),
        }
        assert!(broken.check_structure().is_err(), "mutation {mutation}");
    }
}

#[test]
fn inventory_request_97_rechecks_original_calls_and_uses_the_selected_aware_identity() {
    let (types, bytes, scalar, _) = types();
    let mut original = body(&[bytes, scalar], 1);
    call(&mut original, 0, &[(0, UseKind::Take)], None);
    let fixture = Fixture::named(
        &types,
        original.clone(),
        &[PassingMode::Take],
        "inventory_request_97",
        2,
    );
    let mut definition = witness_definition(original);
    Arc::make_mut(&mut definition).path = DefId::intern(&DefPath::core("inventory_request_97"));
    Arc::make_mut(&mut definition).signature_hash = ContentHash([3; 32]);
    let source = SourceInput::capture(
        ContentHash([1; 32]),
        ContentHash([2; 32]),
        definition.clone(),
    );
    let mut registry = Registry::default();
    let id = register(&mut registry, &fixture, &types);
    assert!(registry.solve().is_err());
    registry
        .resolve(
            id,
            BTreeMap::from([(
                0,
                Expansion::External(ExternalDemand {
                    parameters: vec![0],
                    ..Default::default()
                }),
            )]),
            BTreeMap::new(),
        )
        .unwrap();
    let requests = registry.solve().unwrap();
    let request = &requests[0];
    assert!(request.aware_key().is_some());
    let expected = source.derive_request(request, &types, &[0], Ok).unwrap();
    assert_eq!(
        expected.witness().instance,
        request.aware_key().unwrap().digest()
    );
    assert!(expected.events().is_empty());
    expected.source().check().unwrap();
    let revised = SourceInput::capture(
        ContentHash([1; 32]),
        ContentHash([9; 32]),
        definition.clone(),
    );
    assert_eq!(
        revised.derive_request(request, &types, &[0], Ok).err(),
        Some(SourceError::Malformed(
            "inventory request differs from current original source"
        ))
    );
    Arc::make_mut(&mut definition).bodies[0].blocks[0].terminator = Terminator::Unreachable;
    let changed = SourceInput::capture(ContentHash([1; 32]), ContentHash([2; 32]), definition);
    assert_eq!(
        changed.derive_request(request, &types, &[0], Ok).err(),
        Some(SourceError::Malformed(
            "inventory request differs from current original source"
        ))
    );
}

#[test]
fn inventory_request_97_keeps_unimplemented_retained_mutation_expansion_unavailable() {
    let (types, bytes, scalar, _) = types();
    let mut original = body(&[bytes, scalar], 1);
    call(&mut original, 0, &[(0, UseKind::Mut)], None);
    let fixture = Fixture::named(
        &types,
        original.clone(),
        &[PassingMode::Mut],
        "inventory_mutation_97",
        2,
    );
    let mut definition = witness_definition(original);
    Arc::make_mut(&mut definition).path = DefId::intern(&DefPath::core("inventory_mutation_97"));
    Arc::make_mut(&mut definition).signature_hash = ContentHash([3; 32]);
    let source = SourceInput::capture(ContentHash([1; 32]), ContentHash([2; 32]), definition);
    let mut registry = Registry::default();
    let id = register(&mut registry, &fixture, &types);
    registry
        .resolve(
            id,
            BTreeMap::from([(
                0,
                Expansion::External(ExternalDemand {
                    mutations: vec![(0, vec![0])],
                    ..Default::default()
                }),
            )]),
            BTreeMap::new(),
        )
        .unwrap();
    let requests = registry.solve().unwrap();
    assert_eq!(
        source.derive_request(&requests[0], &types, &[0], Ok).err(),
        Some(SourceError::Unsupported(
            "source witness needs mutation event expansion"
        ))
    );
}
