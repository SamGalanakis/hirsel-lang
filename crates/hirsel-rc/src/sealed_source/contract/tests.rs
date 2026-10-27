//! Original-source DATA assembly controls. These are not V/native witnesses,
//! profile-context selection, complete CT5 coverage or provider activation.
mod registry_tests;
use super::*;
use crate::sealed_source::demand::{CallDemand, ExternalDemand};
use crate::sealed_source::tests::{assign, body, operand, witness_definition};
use crate::sealed_source::{SourceInput, closed_storage_type};
use hirsel_layout::instance::InstanceKey;
use hirsel_span::Span;
use hirsel_syntax::{ContentHash, DefId, DefPath};
use hirsel_tir::*;
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

fn profile(ty: &ClosedStorageType) -> ClosedStorageProfile {
    ClosedStorageProfile {
        schema: ClosedStorageProfile::SCHEMA,
        root: ty.clone(),
        functions: vec![],
    }
}

struct Fixture {
    entry: SourceCallableEntry,
    projection: SourceDemandProjection,
    calls: BTreeMap<u32, CallDemand>,
    closures: BTreeMap<u32, usize>,
    profiles: BTreeMap<Port, ClosedStorageProfile>,
}
impl Fixture {
    fn observed(entry: SourceCallableEntry, projection: SourceDemandProjection) -> Self {
        let mut profiles = entry
            .parameters()
            .iter()
            .map(|p| (Port::Parameter(p.ordinal), profile(&p.ty)))
            .collect::<BTreeMap<_, _>>();
        profiles.insert(Port::Result, profile(entry.result()));
        for capture in entry.captures() {
            profiles.insert(Port::Capture(capture.ordinal), profile(&capture.ty));
        }
        Self {
            entry,
            projection,
            profiles,
            calls: BTreeMap::new(),
            closures: BTreeMap::new(),
        }
    }
    fn named(
        types: &TypeInterner,
        original: Body,
        modes: &[PassingMode],
        label: &str,
        revision: u8,
    ) -> Self {
        let mut definition = witness_definition(original);
        Arc::make_mut(&mut definition).path = DefId::intern(&DefPath::core(label));
        Arc::make_mut(&mut definition).signature_hash = ContentHash([3; 32]);
        let body = &definition.bodies[0];
        let signature = Signature {
            metadata: Default::default(),
            definition: definition.path,
            kind: SignatureKind::Function(function(
                &body
                    .parameters
                    .iter()
                    .zip(modes)
                    .map(|(local, &mode)| (mode, body.locals[local.0 as usize].ty))
                    .collect::<Vec<_>>(),
                body.result,
            )),
            fingerprint: definition.signature_hash,
        };
        assert_eq!(body.parameters.len(), modes.len());
        let key = InstanceKey::new(definition.path.path(), 0, &[], types, None, None).unwrap();
        let source = SourceInput::capture(
            ContentHash([1; 32]),
            ContentHash([revision; 32]),
            definition,
        );
        Self::observed(
            source.named_entry(&key, types, &signature, Ok).unwrap(),
            source.demand_projection(&key).unwrap(),
        )
    }
    fn input(&self) -> Input<'_> {
        Input {
            entry: &self.entry,
            source: &self.projection,
            calls: &self.calls,
            closures: &self.closures,
            profiles: &self.profiles,
        }
    }
    fn derive(&self) -> Result<Declaration, SourceError> {
        Ok(derive(&[self.input()])?.remove(0))
    }
}

fn types() -> (Arc<TypeInterner>, TypeId, TypeId, TypeId) {
    let types = TypeInterner::new(None);
    let bytes = types.intern(Type::with_kind(TypeKind::Primitive(Primitive::Bytes)));
    let scalar = types.intern(Type::with_kind(TypeKind::Primitive(Primitive::U64)));
    let arena = types.intern(Type::with_kind(TypeKind::Named {
        definition: DefId::intern(&DefPath::core("Arena")),
        arguments: vec![].into(),
    }));
    (types, bytes, scalar, arena)
}
fn call(original: &mut Body, block: usize, args: &[(u32, UseKind)], result: Option<u32>) {
    original.blocks[block].terminator = Terminator::Call {
        callee: Callee::Definition {
            path: DefPath::core("original_contract_test_operation"),
            arguments: vec![],
        },
        arguments: args
            .iter()
            .enumerate()
            .map(|(ordinal, &(place, usage))| CallArgument {
                parameter: ParamId(ordinal as u32),
                value: Operand {
                    place: PlaceId(place),
                    usage,
                },
            })
            .collect(),
        returns: result.map(|local| CallReturn {
            destination: PlaceId(local),
            next: BlockId(block as u32 + 1),
        }),
        on_failure: CleanupId(0),
    };
}
fn sink(parameters: &[u32], inventory: &[u32]) -> CallDemand {
    CallDemand::External(ExternalDemand {
        parameters: parameters.to_vec(),
        inventory: inventory.to_vec(),
        ..Default::default()
    })
}

#[test]
fn original_parameter_ordinals_modes_and_arena_effect_are_assembled_canonically() {
    let (types, bytes, scalar, arena) = types();
    let mut original = body(&[bytes, bytes, bytes, arena, scalar], 4);
    call(
        &mut original,
        0,
        &[
            (0, UseKind::Borrow),
            (1, UseKind::Take),
            (2, UseKind::Mut),
            (3, UseKind::Borrow),
        ],
        None,
    );
    let mut fixture = Fixture::named(
        &types,
        original,
        &[
            PassingMode::Borrow,
            PassingMode::Take,
            PassingMode::Mut,
            PassingMode::Borrow,
        ],
        "original_contract_modes",
        2,
    );
    fixture.calls.insert(0, sink(&[0, 1, 2], &[3]));
    let declaration = fixture.derive().unwrap();
    let contract = declaration.contract().unwrap();
    assert_eq!(
        contract
            .parameters
            .iter()
            .map(|p| (p.source, p.mode))
            .collect::<Vec<_>>(),
        [
            (0, ReceiptMode::Borrow),
            (1, ReceiptMode::Take),
            (2, ReceiptMode::MutSlot)
        ]
    );
    assert_eq!(contract.inventory, [ReceiptGrant::Parameter(3)]);
    assert!(contract.result.is_none());
    contract.check_structure().unwrap();
}

#[test]
fn result_demand_crosses_the_original_call_graph_before_contract_assembly() {
    let (types, bytes, _, _) = types();
    let mut identity = body(&[bytes, bytes], 1);
    assign(&mut identity, 1, Rvalue::Use(operand(0)));
    identity.blocks[0].terminator = Terminator::Return {
        value: operand(1),
        cleanup: CleanupId(0),
    };
    let callee = Fixture::named(
        &types,
        identity,
        &[PassingMode::Take],
        "original_contract_id",
        2,
    );
    let mut caller = body(&[bytes, bytes], 1);
    caller.blocks.push(Block {
        instructions: vec![],
        terminator: Terminator::Unreachable,
        span: Span::default(),
    });
    call(&mut caller, 0, &[(0, UseKind::Take)], Some(1));
    call(&mut caller, 1, &[(1, UseKind::Borrow)], None);
    let mut caller = Fixture::named(
        &types,
        caller,
        &[PassingMode::Take],
        "original_contract_caller",
        2,
    );
    caller.calls.insert(0, CallDemand::Bodies(vec![0]));
    caller.calls.insert(1, sink(&[0], &[]));
    let declarations = derive(&[callee.input(), caller.input()]).unwrap();
    let contract = declarations[0].contract().unwrap();
    assert_eq!(contract.parameters[0].mode, ReceiptMode::Take);
    assert_eq!(
        contract.result.as_ref().unwrap().ty,
        contract.parameters[0].ty
    );
    assert!(declarations[1].contract().unwrap().result.is_none());
}

#[test]
fn independently_empty_requirements_differ_from_missing_call_expansion() {
    let (types, bytes, _, _) = types();
    let mut original = body(&[bytes, bytes], 1);
    call(&mut original, 0, &[(0, UseKind::Borrow)], Some(1));
    let mut fixture = Fixture::named(
        &types,
        original,
        &[PassingMode::Borrow],
        "original_contract_empty",
        2,
    );
    assert_eq!(
        fixture.derive().unwrap_err(),
        SourceError::Unsupported("demand needs complete original call expansion")
    );
    fixture
        .calls
        .insert(0, CallDemand::External(ExternalDemand::default()));
    fixture.profiles.clear();
    assert!(fixture.derive().unwrap().contract().is_none());
}

#[test]
fn internal_demand_without_boundary_carriers_is_observed_some_empty() {
    let (types, bytes, scalar, _) = types();
    let mut original = body(&[bytes, scalar], 0);
    assign(
        &mut original,
        0,
        Rvalue::Constant(Constant::Bytes(vec![1, 2, 3])),
    );
    call(&mut original, 0, &[(0, UseKind::Borrow)], None);
    let mut fixture = Fixture::named(&types, original, &[], "original_contract_internal", 2);
    fixture.calls.insert(0, sink(&[0], &[]));
    fixture.profiles.clear();
    let declaration = fixture.derive().unwrap();
    assert_eq!(declaration.requirements().payload_locals, [0].into());
    assert!(declaration.contract().unwrap().is_empty());
}

#[test]
fn missing_wrong_and_nonexistent_profile_ports_never_become_empty_contracts() {
    let (types, bytes, scalar, _) = types();
    let mut original = body(&[bytes, scalar], 1);
    call(&mut original, 0, &[(0, UseKind::Borrow)], None);
    let mut fixture = Fixture::named(
        &types,
        original,
        &[PassingMode::Borrow],
        "original_contract_profile",
        2,
    );
    fixture.calls.insert(0, sink(&[0], &[]));
    let real = fixture.profiles.remove(&Port::Parameter(0)).unwrap();
    assert_eq!(
        fixture.derive().unwrap_err(),
        SourceError::Unsupported("receipt demand needs independently observed closed profile")
    );
    fixture.profiles.insert(
        Port::Parameter(0),
        profile(&closed_storage_type(&types, scalar).unwrap()),
    );
    assert_eq!(
        fixture.derive().unwrap_err(),
        SourceError::Malformed("receipt profile differs from full original source type")
    );
    fixture.profiles.insert(Port::Parameter(0), real.clone());
    fixture.profiles.insert(Port::Parameter(9), real);
    assert_eq!(
        fixture.derive().unwrap_err(),
        SourceError::Malformed("receipt profile has no original entry selector")
    );
}

#[test]
fn source_revision_binding_is_separate_from_stable_semantic_demand_content() {
    let (types, bytes, scalar, _) = types();
    let mut original = body(&[bytes, scalar], 1);
    call(&mut original, 0, &[(0, UseKind::Borrow)], None);
    let mut before = Fixture::named(
        &types,
        original.clone(),
        &[PassingMode::Borrow],
        "original_contract_revision",
        2,
    );
    let mut after = Fixture::named(
        &types,
        original,
        &[PassingMode::Borrow],
        "original_contract_revision",
        9,
    );
    before.calls.insert(0, sink(&[0], &[]));
    after.calls = before.calls.clone();
    assert_ne!(
        before.entry.implementation_revision(),
        after.entry.implementation_revision()
    );
    assert_eq!(
        before.derive().unwrap().contract(),
        after.derive().unwrap().contract()
    );
    before.projection = after.projection;
    assert_eq!(
        before.derive().unwrap_err(),
        SourceError::Malformed("receipt entry and demand source observation differ")
    );
}

#[test]
fn ordinary_and_unique_generic_actuals_sharing_erased_base_cannot_share_source_profile() {
    let (types, bytes, _, _) = types();
    let scope = body(&[bytes], 1).parameter_scope;
    let bound = types.intern(Type::with_kind(TypeKind::Parameter(
        TypeParameterRef::bound(TypeParamId(0)),
    )));
    let parameter = types.intern(Type::with_kind(TypeKind::Parameter(
        TypeParameterRef::body(scope, TypeParamId(0)),
    )));
    let mut original = body(&[parameter, parameter], 1);
    call(&mut original, 0, &[(0, UseKind::Borrow)], None);
    let mut definition = witness_definition(original);
    Arc::make_mut(&mut definition).signature_hash = ContentHash([3; 32]);
    let mut generic = function(&[(PassingMode::Take, bound)], bound);
    generic.generics = vec![GenericParameter::Type {
        name: "T".into(),
        bounds: Default::default(),
    }]
    .into();
    let signature = Signature {
        metadata: Default::default(),
        definition: definition.path,
        kind: SignatureKind::Function(generic),
        fingerprint: definition.signature_hash,
    };
    let source = SourceInput::capture(
        ContentHash([1; 32]),
        ContentHash([2; 32]),
        definition.clone(),
    );
    let ordinary = crate::instance::Actuals {
        binder: Some(scope),
        arguments: vec![GenericArgument::Type(bytes)],
        outer: vec![],
    };
    let mut unique = types.get(bytes).unwrap().clone();
    unique.modes.uniqueness = Uniqueness::Unique;
    let unique = types.intern(unique);
    let selected = crate::instance::Actuals {
        arguments: vec![GenericArgument::Type(unique)],
        ..ordinary.clone()
    };
    let key = crate::instance::key_of(
        &types,
        definition.path.path(),
        BodyId(0),
        &ordinary,
        None,
        None,
    )
    .unwrap();
    assert_eq!(
        key,
        crate::instance::key_of(
            &types,
            definition.path.path(),
            BodyId(0),
            &selected,
            None,
            None
        )
        .unwrap()
    );
    let entry = |actuals| {
        source
            .named_entry_selected(&key, &types, &signature, actuals, &TypeMemo::new())
            .unwrap()
    };
    let mut ordinary = Fixture::observed(entry(&ordinary), source.demand_projection(&key).unwrap());
    let mut selected = Fixture::observed(entry(&selected), source.demand_projection(&key).unwrap());
    ordinary.calls.insert(0, sink(&[0], &[]));
    selected.calls = ordinary.calls.clone();
    let a = ordinary.derive().unwrap();
    let b = selected.derive().unwrap();
    assert_ne!(
        a.contract().unwrap().parameters[0].ty,
        b.contract().unwrap().parameters[0].ty
    );
    assert_eq!(
        a.entry().parameters()[0].ty.ty,
        b.entry().parameters()[0].ty.ty
    );
    selected.profiles.insert(
        Port::Parameter(0),
        ordinary.profiles[&Port::Parameter(0)].clone(),
    );
    assert_eq!(
        selected.derive().unwrap_err(),
        SourceError::Malformed("receipt profile differs from full original source type")
    );
}

fn captured_fixture(usage: UseKind, arena: bool) -> Fixture {
    let (types, bytes, scalar, grant) = types();
    let captured = if arena { grant } else { bytes };
    let callable = types.intern(Type::with_kind(TypeKind::Function(function(
        &[(PassingMode::Borrow, captured)],
        scalar,
    ))));
    let mut parent = body(&[captured, callable], 1);
    assign(
        &mut parent,
        1,
        Rvalue::Closure {
            body: BodyId(1),
            captures: vec![Operand {
                place: PlaceId(0),
                usage,
            }],
            context: CaptureContext::Scoped,
            destination: Destination::Heap,
        },
    );
    let mut child = body(&[captured, captured, scalar], 1);
    child.locals[1].storage = Storage::Capture;
    call(&mut child, 0, &[(1, UseKind::Borrow)], None);
    let mut definition = witness_definition(parent);
    Arc::make_mut(&mut definition).bodies.push(child);
    let key = InstanceKey::new(definition.path.path(), 1, &[], &types, None, None).unwrap();
    let source = SourceInput::capture(ContentHash([1; 32]), ContentHash([2; 32]), definition);
    let entry = source
        .closure_entry(&key, &types, BodyId(0), BlockId(0), 0, Ok)
        .unwrap();
    let mut fixture = Fixture::observed(entry, source.demand_projection(&key).unwrap());
    fixture.calls.insert(0, sink(&[0], &[]));
    fixture
}

#[test]
fn demanded_borrowed_capture_uses_original_ordinal_and_complete_profile() {
    let fixture = captured_fixture(UseKind::Borrow, false);
    let declaration = fixture.derive().unwrap();
    let contract = declaration.contract().unwrap();
    assert!(contract.parameters.is_empty());
    assert!(contract.result.is_none());
    assert!(contract.inventory.is_empty());
    assert_eq!(
        contract.captures,
        vec![ReceiptCapture {
            ordinal: 0,
            ty: ReceiptType {
                source_type: fixture.entry.captures()[0].ty.source_type,
                profile: fixture.profiles[&Port::Capture(0)].content().unwrap(),
            },
            mode: ReceiptMode::Borrow,
        }]
    );
    contract.check_structure().unwrap();
}

#[test]
fn demanded_borrowed_capture_requires_a_complete_profile() {
    let mut fixture = captured_fixture(UseKind::Borrow, false);
    fixture.profiles.remove(&Port::Capture(0));
    assert_eq!(
        fixture.derive().unwrap_err(),
        SourceError::Unsupported("receipt demand needs independently observed closed profile")
    );
}

#[test]
fn demanded_borrowed_capture_rejects_another_original_profile() {
    let mut fixture = captured_fixture(UseKind::Borrow, false);
    fixture
        .profiles
        .insert(Port::Capture(0), fixture.profiles[&Port::Result].clone());
    assert_eq!(
        fixture.derive().unwrap_err(),
        SourceError::Malformed("receipt profile differs from full original source type")
    );
}

#[test]
fn demanded_copy_and_take_captures_declare_owning_metadata_with_complete_profiles() {
    for usage in [UseKind::Take, UseKind::Copy] {
        let mut fixture = captured_fixture(usage, false);
        let declaration = fixture.derive().unwrap();
        let capture = &declaration.contract().unwrap().captures[0];
        assert_eq!(capture.ordinal, 0);
        assert_eq!(capture.mode, ReceiptMode::Take);
        assert_eq!(
            capture.ty.source_type,
            fixture.entry.captures()[0].ty.source_type
        );
        assert_eq!(
            capture.ty.profile,
            fixture.profiles[&Port::Capture(0)].content().unwrap()
        );
        fixture.profiles.remove(&Port::Capture(0));
        assert_eq!(
            fixture.derive().unwrap_err(),
            SourceError::Unsupported("receipt demand needs independently observed closed profile")
        );
    }
}

#[test]
fn demanded_mutable_capture_requires_exclusive_environment_transport() {
    let fixture = captured_fixture(UseKind::Mut, false);
    assert_eq!(
        fixture.derive().unwrap_err(),
        SourceError::Unsupported("receipt mutable capture needs exclusive environment transport")
    );
}

#[test]
fn owning_inventory_captures_require_original_grant_transport() {
    for usage in [UseKind::Take, UseKind::Copy] {
        let fixture = captured_fixture(usage, false);
        fixture.derive().unwrap();
        let mut grant = captured_fixture(usage, true);
        grant.calls.insert(0, sink(&[], &[0]));
        assert_eq!(
            grant.derive().unwrap_err(),
            SourceError::Unsupported(
                "receipt owning inventory capture needs original grant transport"
            )
        );
    }
}

#[test]
fn undemanded_borrowed_capture_keeps_the_ordinary_contract() {
    let mut fixture = captured_fixture(UseKind::Borrow, false);
    fixture.calls.insert(0, sink(&[], &[]));
    assert!(fixture.derive().unwrap().contract().is_none());
}

#[test]
fn demanded_capture_inventory_names_only_its_original_arena() {
    let mut fixture = captured_fixture(UseKind::Borrow, true);
    fixture.calls.insert(0, sink(&[], &[0]));
    let declaration = fixture.derive().unwrap();
    let contract = declaration.contract().unwrap();
    assert!(contract.parameters.is_empty());
    assert!(contract.captures.is_empty());
    assert_eq!(contract.inventory, vec![ReceiptGrant::Capture(0)]);
    contract.check_structure().unwrap();
}

#[test]
fn demanded_capture_inventory_cannot_retag_a_nongrant() {
    let mut fixture = captured_fixture(UseKind::Borrow, false);
    fixture.calls.insert(0, sink(&[], &[0]));
    assert_eq!(
        fixture.derive().unwrap_err(),
        SourceError::Unsupported("receipt inventory needs exact original Arena grant")
    );
}

#[test]
fn inventory_demand_cannot_turn_a_nongrant_parameter_into_an_arena_effect() {
    let (types, bytes, scalar, _) = types();
    let mut original = body(&[bytes, scalar], 1);
    call(&mut original, 0, &[(0, UseKind::Borrow)], None);
    let mut fixture = Fixture::named(
        &types,
        original,
        &[PassingMode::Borrow],
        "original_contract_nongrant",
        2,
    );
    fixture.calls.insert(0, sink(&[], &[0]));
    assert_eq!(
        fixture.derive().unwrap_err(),
        SourceError::Unsupported("receipt inventory needs exact original Arena grant")
    );
}

#[test]
fn returned_grant_demand_remains_strict_until_original_grant_transport_exists() {
    let (types, _, _, arena) = types();
    let mut identity = body(&[arena, arena], 1);
    assign(&mut identity, 1, Rvalue::Use(operand(0)));
    identity.blocks[0].terminator = Terminator::Return {
        value: operand(1),
        cleanup: CleanupId(0),
    };
    let callee = Fixture::named(
        &types,
        identity,
        &[PassingMode::Take],
        "original_contract_grant_id",
        2,
    );
    let mut caller = body(&[arena, arena], 1);
    caller.blocks.push(Block {
        instructions: vec![],
        terminator: Terminator::Unreachable,
        span: Span::default(),
    });
    call(&mut caller, 0, &[(0, UseKind::Take)], Some(1));
    call(&mut caller, 1, &[(1, UseKind::Borrow)], None);
    let mut caller = Fixture::named(
        &types,
        caller,
        &[PassingMode::Take],
        "original_contract_grant_caller",
        2,
    );
    caller.calls.insert(0, CallDemand::Bodies(vec![0]));
    caller.calls.insert(1, sink(&[], &[0]));
    assert_eq!(
        derive(&[callee.input(), caller.input()]).unwrap_err(),
        SourceError::Unsupported("receipt result needs original grant transport")
    );
}
