//! Quotient-graph and address DATA controls. Actual source/profile/target
//! completeness remains independent; these fixtures do not manufacture V proof.
use super::*;
use crate::sealed_source::contract::registry::{Context, ContextId, Expansion, Registry};

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
            profile: &fixture.profiles[&Port::Parameter(0)],
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
        &[&fixture.profiles[&Port::Parameter(0)]
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
                profile: &a.profiles[&Port::Parameter(0)],
                entry: &b.entry,
                source: &b.projection,
                ports: &b.profiles
            })
            .unwrap_err(),
        SourceError::Malformed("conflicting original data for receipt profile context")
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
                profile: &b.profiles[&Port::Parameter(0)],
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
                    profile: &fixture.profiles[&Port::Parameter(0)],
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
