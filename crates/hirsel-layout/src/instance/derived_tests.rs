//! Canonical key DATA identities, not source/profile completeness or authority.
use super::*;
use hirsel_lir::operation::{NativeType, Primitive as NativePrimitive};
use hirsel_lir::sealed_payload_source::{
    ClosedStorageProfile, ClosedStorageType, ReceiptContract, ReceiptMode, ReceiptParameter,
    ReceiptType,
};
use hirsel_types::{GenericArgument, Primitive, Type, TypeInterner, TypeKind};
use std::collections::HashMap;

const PROFILE: &str = "hirsel.closed-storage-profile.v1";
const DEMAND: &str = "hirsel.receipt-demand.v1";

fn base(name: &str, body: u32) -> InstanceKey {
    InstanceKey::new(
        &DefPath::core(name),
        body,
        &[],
        &TypeInterner::new(None),
        None,
        None,
    )
    .unwrap()
}

#[test]
fn identity_shape_preserves_scoped_environment_and_direct_nested_convention() {
    use hirsel_lir::abi_types::CallMultiplicity;
    let types = TypeInterner::new(None);
    let ty = types.intern(Type::with_kind(TypeKind::Primitive(Primitive::U64)));
    let actuals = [(
        ActualBinder::Type(hirsel_types::TypeParameterRef::bound(
            hirsel_types::TypeParamId(0),
        )),
        GenericArgument::Type(ty),
    )];
    let conventions = [
        None,
        Some(CallableConvention {
            parameters: vec![],
            multiplicity: CallMultiplicity::Many,
        }),
        Some(CallableConvention {
            parameters: vec![Passing::Mut, Passing::Take, Passing::Borrow],
            multiplicity: CallMultiplicity::Once,
        }),
    ];
    let scoped = ContentHash::of_bytes(b"scoped capture identity, not physical layout identity");
    for convention in conventions {
        for environment in [None, Some(scoped)] {
            let path = DefPath::core("identity.scoped_shape");
            let key =
                InstanceKey::new(&path, 3, &actuals, &types, convention.as_ref(), environment)
                    .unwrap();
            let restored = InstanceKey::from_encoding(&path, 3, key.encoding().to_vec()).unwrap();
            assert_eq!(
                key.identity_shape().unwrap(),
                (convention.clone(), environment)
            );
            assert_eq!(
                restored.identity_shape().unwrap(),
                (convention.clone(), environment)
            );
            assert_eq!(
                key.derived("identity.stage", &[b"stage-input"])
                    .identity_shape()
                    .unwrap(),
                (convention.clone(), environment)
            );
        }
    }
}

#[test]
fn identity_shape_refuses_missing_environment_framing_and_wrong_parameter_count() {
    use hirsel_lir::abi_types::CallMultiplicity;
    let key = base("identity.missing_shape", 0);
    let mut bytes = key.encoding().to_vec();
    bytes.truncate(bytes.len() - 9);
    let malformed = InstanceKey::from_encoding(key.definition(), key.body(), bytes).unwrap();
    assert_eq!(malformed.identity_shape(), Err(InstanceKeyError::Encoding));

    let convention = CallableConvention {
        parameters: vec![Passing::Mut],
        multiplicity: CallMultiplicity::Many,
    };
    let key = InstanceKey::new(
        key.definition(),
        0,
        &[],
        &TypeInterner::new(None),
        Some(&convention),
        None,
    )
    .unwrap();
    let mut parts = atoms(key.encoding());
    let bad_count = 2u64.to_le_bytes();
    let position = parts.iter().position(|atom| *atom == b"mut").unwrap() - 1;
    parts[position] = &bad_count;
    let mut bytes = Vec::new();
    for part in parts {
        bytes.extend_from_slice(&(part.len() as u64).to_le_bytes());
        bytes.extend_from_slice(part);
    }
    let malformed = InstanceKey::from_encoding(key.definition(), key.body(), bytes).unwrap();
    assert_eq!(malformed.identity_shape(), Err(InstanceKeyError::Encoding));
}

fn atoms(mut bytes: &[u8]) -> Vec<&[u8]> {
    let mut parts = Vec::new();
    while !bytes.is_empty() {
        let length = u64::from_le_bytes(bytes[..8].try_into().unwrap());
        let (part, tail) = bytes[8..].split_at(usize::try_from(length).unwrap());
        parts.push(part);
        bytes = tail;
    }
    parts
}

fn profile(source: u8) -> ClosedStorageProfile {
    ClosedStorageProfile {
        schema: ClosedStorageProfile::SCHEMA,
        root: ClosedStorageType {
            ty: NativeType::Primitive(NativePrimitive::Bytes),
            source_type: ContentHash([source; 32]),
        },
        functions: vec![],
    }
    .canonical()
    .unwrap()
}

fn contract(profile: &ClosedStorageProfile, mode: ReceiptMode) -> ReceiptContract {
    ReceiptContract::new(
        vec![ReceiptParameter {
            source: 0,
            mode,
            ty: ReceiptType {
                source_type: profile.root.source_type,
                profile: profile.content().unwrap(),
            },
        }],
        None,
        vec![],
        vec![],
    )
    .unwrap()
}

#[test]
fn derived_key_preserves_base_owner_bytes_and_existing_digest_operation() {
    let ordinary = base("derived.identity", 7);
    let before = ordinary.encoding().to_vec();
    let digest = ordinary.digest();
    let derived = ordinary.derived(DEMAND, &[b"parameter-zero", b"\0binary\xff"]);
    assert_eq!(ordinary.encoding(), before);
    assert_eq!(ordinary.digest(), digest);
    assert_eq!(derived.definition(), ordinary.definition());
    assert_eq!(derived.body(), 7);
    assert_ne!(derived, ordinary);
    assert_eq!(
        derived.digest(),
        ContentHash::of_parts(
            DEMAND,
            [digest.0.as_slice(), b"parameter-zero", b"\0binary\xff"]
        )
    );
    assert_eq!(
        atoms(derived.encoding()),
        [
            b"hirsel.derived-instance.v1".as_slice(),
            before.as_slice(),
            DEMAND.as_bytes(),
            b"parameter-zero",
            b"\0binary\xff"
        ]
    );
}

#[test]
fn framing_distinguishes_atom_boundaries_and_absent_versus_empty_inputs() {
    let ordinary = base("derived.framing", 0);
    let a = ordinary.derived(DEMAND, &[b"a", b"bc"]);
    let b = ordinary.derived(DEMAND, &[b"ab", b"c"]);
    assert_ne!(a, b);
    assert_ne!(a.digest(), b.digest());
    let absent = ordinary.derived(DEMAND, &[]);
    let empty = ordinary.derived(DEMAND, &[b""]);
    assert_ne!(absent, empty);
    assert_ne!(absent.digest(), empty.digest());
    assert_eq!(atoms(absent.encoding()).len(), 3);
    assert_eq!(
        atoms(empty.encoding()).last().copied(),
        Some(b"".as_slice())
    );
}

#[test]
fn namespaces_and_parent_instances_are_separate_full_identity_inputs() {
    let ordinary = base("derived.namespaces", 0);
    let profile = ordinary.derived(PROFILE, &[b"same bytes"]);
    let demand = ordinary.derived(DEMAND, &[b"same bytes"]);
    assert_ne!(profile, demand);
    assert_ne!(profile.digest(), demand.digest());
    assert_ne!(
        demand,
        base("derived.other", 0).derived(DEMAND, &[b"same bytes"])
    );
    assert_ne!(
        demand,
        base("derived.namespaces", 1).derived(DEMAND, &[b"same bytes"])
    );
}

#[test]
fn canonical_profile_then_receipt_composition_retains_the_complete_parent() {
    let ordinary = base("derived.composed", 0);
    let profile = profile(1);
    let contract = contract(&profile, ReceiptMode::Take);
    let profile_bytes = profile.canonical_bytes().unwrap();
    let contract_bytes = contract.canonical_bytes().unwrap();
    let profiled = ordinary.derived(PROFILE, &[&profile_bytes]);
    let demanded = profiled.derived(DEMAND, &[&contract_bytes]);
    assert_eq!(atoms(demanded.encoding())[1], profiled.encoding());
    assert_eq!(atoms(profiled.encoding())[1], ordinary.encoding());
    assert_eq!(
        demanded.digest(),
        ContentHash::of_parts(
            DEMAND,
            [profiled.digest().0.as_slice(), contract_bytes.as_slice()]
        )
    );
    let flattened = ordinary.derived(DEMAND, &[&profile_bytes, &contract_bytes]);
    assert_ne!(demanded, flattened);
    assert_ne!(demanded.digest(), flattened.digest());
    assert_eq!(
        demanded,
        ordinary
            .derived(PROFILE, &[&profile_bytes])
            .derived(DEMAND, &[&contract_bytes])
    );
}

#[test]
fn same_erased_base_retains_distinct_full_source_profile_contexts() {
    let ordinary = base("derived.full_source", 0);
    let a = profile(1);
    let b = profile(2);
    assert_eq!(a.root.ty, b.root.ty);
    assert_ne!(a.root.source_type, b.root.source_type);
    let a_profile = ordinary.derived(PROFILE, &[&a.canonical_bytes().unwrap()]);
    let b_profile = ordinary.derived(PROFILE, &[&b.canonical_bytes().unwrap()]);
    assert_ne!(a_profile, b_profile);
    let a_contract = contract(&a, ReceiptMode::Borrow);
    let b_contract = contract(&b, ReceiptMode::Borrow);
    assert_ne!(
        a_profile.derived(DEMAND, &[&a_contract.canonical_bytes().unwrap()]),
        b_profile.derived(DEMAND, &[&b_contract.canonical_bytes().unwrap()])
    );
}

#[test]
fn derived_identity_is_portable_across_interner_row_order() {
    let first = TypeInterner::new(None);
    let a = first.intern(Type::with_kind(TypeKind::Primitive(Primitive::U64)));
    let second = TypeInterner::new(None);
    second.intern(Type::with_kind(TypeKind::Primitive(Primitive::Bytes)));
    let b = second.intern(Type::with_kind(TypeKind::Primitive(Primitive::U64)));
    assert_ne!(a, b);
    let binder = ActualBinder::Type(hirsel_types::TypeParameterRef::bound(
        hirsel_types::TypeParamId(0),
    ));
    let path = DefPath::core("derived.portable");
    let a = InstanceKey::new(
        &path,
        0,
        &[(binder, GenericArgument::Type(a))],
        &first,
        None,
        None,
    )
    .unwrap();
    let b = InstanceKey::new(
        &path,
        0,
        &[(binder, GenericArgument::Type(b))],
        &second,
        None,
        None,
    )
    .unwrap();
    assert_eq!(a, b);
    assert_eq!(
        a.derived(PROFILE, &[b"canonical profile"])
            .derived(DEMAND, &[b"canonical contract"]),
        b.derived(PROFILE, &[b"canonical profile"])
            .derived(DEMAND, &[b"canonical contract"])
    );
}

#[test]
fn equal_parent_digests_do_not_erase_complete_parent_encoding_in_equality() {
    let first = base("derived.parent_a", 0);
    let mut second = base("derived.parent_b", 0);
    // Synthetic hash collision checks the equality contract; no public caller
    // can mutate the private digest, and no source authority follows from it.
    second.digest = first.digest;
    let a = first.derived(DEMAND, &[b"same input"]);
    let b = second.derived(DEMAND, &[b"same input"]);
    assert_eq!(a.digest(), b.digest());
    assert_ne!(a.encoding(), b.encoding());
    assert_ne!(a, b);
    let map = HashMap::from([(a.clone(), 1), (b.clone(), 2)]);
    assert_eq!(map.len(), 2);
    assert_eq!(map.get(&a), Some(&1));
    assert_eq!(map.get(&b), Some(&2));
}
