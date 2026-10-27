//! Hand-built calls, independent of RC/import. Every mutation is otherwise valid
//! and validates when the operation-declaration ABI check is removed.
use super::support::{self, ENTRY, Lir};
use hirsel_core_contract::{Manifest, OperationId, Passing};
use hirsel_layout::layout::{OwnerRepresentation, Owners};
use hirsel_lir::abi_types::FnAbiId;
use hirsel_lir::ir::*;

use hirsel_layout::operation::OperationRegistry;
use hirsel_lir::layout_types::*;
use hirsel_lir::operation::{NativeType, OperationArgument as A, OperationDeclarations};
use hirsel_lir::stage::{Stage, Value};
use hirsel_lir::validate::{Inputs, ViolationKind as K};
use hirsel_syntax::{ContentHash, DefId, DefPath, ItemPath, PackageId, PassingMode};
use hirsel_tir::RepresentationRecord;
use hirsel_types::*;
use std::sync::Arc;

fn path(name: &str) -> DefPath {
    let mut parts: Vec<_> = name.split('.').map(str::to_owned).collect();
    DefPath(ItemPath {
        package: PackageId {
            name: "core".into(),
            version: "1".into(),
        },
        name: parts.pop().unwrap(),
        modules: parts,
    })
}

fn ty(store: &TypeInterner, kind: TypeKind) -> TypeId {
    store.intern(Type {
        kind,
        ..Type::unit()
    })
}

fn function(
    parameters: Vec<Parameter>,
    result: TypeId,
    generics: Vec<GenericParameter>,
) -> FunctionType {
    FunctionType {
        state_promises: Default::default(),
        parameters: parameters.into(),
        result,
        generics: generics.into(),
        error: None,
        allocation: Allocation::NoAlloc,
        authority: AuthoritySummary::Unknown,
        uses: Uses::Unrestricted,
        contracts: Default::default(),
        incomplete: false,
        foreign: None,
    }
}

pub(super) fn signature(name: &str, types: &TypeInterner, f: FunctionType) -> SignatureRef {
    let mut s = Signature {
        metadata: Default::default(),
        definition: DefId::intern(&path(name)),
        kind: SignatureKind::Function(f),
        fingerprint: Default::default(),
    };
    s.fingerprint = encoding::fingerprint(types, &s, &[]).unwrap();
    Arc::new(s)
}

pub(super) fn id(s: &Signature) -> OperationId {
    OperationId {
        definition: s.definition.path().clone(),
        declaration_digest: s.fingerprint,
    }
}

fn parameter(ty: TypeId, passing: PassingMode) -> Parameter {
    Parameter {
        name: None,
        label: None,
        ty,
        passing,
    }
}

pub(super) fn scalar_signature(
    name: &str,
    params: &[(PassingMode, Primitive)],
    result: Option<Primitive>,
) -> SignatureRef {
    let store = support::session();
    let parameters = params
        .iter()
        .map(|(passing, p)| {
            let t = ty(&store, TypeKind::Primitive(*p));
            parameter(t, *passing)
        })
        .collect();
    let result = match result {
        Some(p) => ty(&store, TypeKind::Primitive(p)),
        None => store.intern(Type::unit()),
    };
    signature(name, &store, function(parameters, result, vec![]))
}

fn nominal(owners: &mut Owners, name: &str, generics: Vec<GenericParameter>, shape: TypeId) {
    owners.nominals.insert(
        DefId::intern(&path(name)),
        OwnerRepresentation::Defined(RepresentationRecord {
            definition: path(name),
            representation_revision: ContentHash::of_bytes(name.as_bytes()),
            generics,
            private_shape: shape,
            layout_promises: Layout::default(),
            resource: false,
            finalizer: None,
            unsafe_ffi_package: false,
        }),
    );
}

pub(super) fn ready_signature(result: bool) -> (SignatureRef, Owners) {
    if !result {
        return (scalar_signature("fixture", &[], None), Owners::default());
    }
    let mut owners = Owners::default();
    let store = support::session();
    let shape = ty(&store, TypeKind::Primitive(Primitive::I64));
    nominal(&mut owners, "ResultCell", vec![], shape);
    let store = support::session();
    let result = ty(
        &store,
        TypeKind::Named {
            definition: DefId::intern(&path("ResultCell")),
            arguments: Default::default(),
        },
    );
    (
        signature("fixture", &store, function(vec![], result, vec![])),
        owners,
    )
}

fn network() -> (OperationRegistry, OperationId, OperationId) {
    let mut owners = Owners::default();
    for (name, resource) in [("Net", false), ("Stream", true)] {
        owners.nominals.insert(
            DefId::intern(&path(name)),
            OwnerRepresentation::Handle {
                revision: ContentHash::of_bytes(name.as_bytes()),
                resource,
            },
        );
    }
    let store = support::session();
    let word = ty(&store, TypeKind::Primitive(Primitive::U32));
    let error = ty(
        &store,
        TypeKind::Variant(
            vec![Variant {
                name: "Io".into(),
                fields: vec![(None, word), (None, word), (None, word)].into(),
            }]
            .into(),
        ),
    );
    nominal(&mut owners, "NetError", vec![], error);
    let store = support::session();
    let t = ty(
        &store,
        TypeKind::Parameter(TypeParameterRef::bound(TypeParamId(0))),
    );
    let e = ty(
        &store,
        TypeKind::Parameter(TypeParameterRef::bound(TypeParamId(1))),
    );
    let result = ty(
        &store,
        TypeKind::Variant(
            vec![
                Variant {
                    name: "Ok".into(),
                    fields: vec![(None, t)].into(),
                },
                Variant {
                    name: "Error".into(),
                    fields: vec![(None, e)].into(),
                },
            ]
            .into(),
        ),
    );
    let generic = |name: &str| GenericParameter::Type {
        name: name.into(),
        bounds: Default::default(),
    };
    nominal(
        &mut owners,
        "Result",
        vec![generic("T"), generic("E")],
        result,
    );
    let mut registry = OperationRegistry {
        seal_sources: Default::default(),
        receipt_sources: Default::default(),
        pretransfer_adapters: Vec::new(),
        pretransfer_actuals: Vec::new(),
        invocation_bodies: Vec::new(),
        owners,
        ..support::registry()
    };
    let mut ids = Vec::new();
    for intrinsic in [false, true] {
        let store = support::session();
        let net = ty(
            &store,
            TypeKind::Named {
                definition: DefId::intern(&path("Net")),
                arguments: Default::default(),
            },
        );
        let stream = ty(
            &store,
            TypeKind::Named {
                definition: DefId::intern(&path("Stream")),
                arguments: Default::default(),
            },
        );
        let byte = ty(&store, TypeKind::Primitive(Primitive::U8));
        let mut arguments = vec![GenericArgument::Type(byte)];
        if intrinsic {
            arguments.push(GenericArgument::Size(Size::Parameter(
                SizeParameterRef::bound(SizeParamId(0)),
            )));
        }
        let buffer = ty(
            &store,
            TypeKind::Builtin {
                kind: if intrinsic {
                    BuiltinType::Inline
                } else {
                    BuiltinType::List
                },
                arguments: arguments.into(),
            },
        );
        let n = ty(&store, TypeKind::Primitive(Primitive::U64));
        let error = ty(
            &store,
            TypeKind::Named {
                definition: DefId::intern(&path("NetError")),
                arguments: Default::default(),
            },
        );
        let result = ty(
            &store,
            TypeKind::Named {
                definition: DefId::intern(&path("Result")),
                arguments: vec![GenericArgument::Type(n), GenericArgument::Type(error)].into(),
            },
        );
        let generics = if intrinsic {
            vec![GenericParameter::Size { name: "k".into() }]
        } else {
            vec![]
        };
        let s = signature(
            if intrinsic {
                "$intrinsic.NetReadInto"
            } else {
                "net.read"
            },
            &store,
            function(
                vec![
                    parameter(net, PassingMode::Borrow),
                    parameter(stream, PassingMode::Mut),
                    parameter(buffer, PassingMode::Mut),
                    parameter(n, PassingMode::Borrow),
                ],
                result,
                generics,
            ),
        );
        ids.push(id(&s));
        registry.signatures.insert(id(&s), s);
    }
    (registry, ids[0].clone(), ids[1].clone())
}

fn repr(table: &LayoutTable, layout: LayoutId) -> ValueRepr {
    match table.layout(layout).unwrap().kind {
        LayoutKind::Unit => ValueRepr::Unit,
        LayoutKind::Scalar(k) => ValueRepr::Scalar(k),
        LayoutKind::Block { .. } | LayoutKind::Box { .. } => ValueRepr::Block,
        LayoutKind::Handle | LayoutKind::HostCapability { .. } | LayoutKind::Path => {
            ValueRepr::Handle
        }
        LayoutKind::Arena => ValueRepr::Arena,
        LayoutKind::Callable { .. } => ValueRepr::Callable,
        _ => ValueRepr::Aggregate,
    }
}

/// Forward the entry's parameters and result through one operation call.
fn fixture(
    registry: &OperationRegistry,
    operation: &OperationId,
    actuals: Vec<A>,
) -> (LirUnit<Value>, LayoutTable) {
    let target = TargetSpec::x86_64_unknown_linux_gnu();
    let (abi, table) = registry.derive(operation, &actuals, &target).unwrap();
    let mut b = Lir::<Value>::new(&[], None);
    b.table = table;
    b.builder.unit.header.abi = FnAbiId(0);
    b.builder.unit.tables.operations = vec![operation.clone()];
    let mut arguments = Vec::new();
    for p in &abi.parameters {
        let repr = repr(&b.table, p.layout);
        let v = b.value_as(repr, p.layout, Holding::Plain);
        b.param(v);
        let value = if p.convention == Passing::Mut {
            let t = b.builder.unit.values[v.index()].ty;
            let slot = SlotId(b.builder.unit.slots.len() as u32);
            b.builder.unit.slots.push(SlotInfo {
                ty: t,
                kind: SlotKind::Parameter(p.source),
                scope: support::ROOT,
            });
            let address = AddressId(b.builder.unit.addresses.len() as u32);
            b.builder.unit.addresses.push(AddressInfo {
                source_place: None,
                unchecked: Vec::new(),
                ty: t,
                base: AddressBase::Slot(slot),
                projection: PoolRange::EMPTY,
            });
            b.prepare(ENTRY, address, &[]);
            let version = b.value_as(repr, p.layout, Holding::Plain);
            ArgumentValue::Address { address, version }
        } else {
            ArgumentValue::Value(v)
        };
        arguments.push(CallArgument {
            parameter: p.source,
            passing: p.convention,
            value,
        });
    }
    let result = abi
        .result
        .as_ref()
        .map(|r| b.value_as(repr(&b.table, r.layout), r.layout, Holding::Plain));
    let arguments = PoolRange::push(&mut b.builder.unit.pools.call_arguments, arguments);
    b.builder.unit.tables.calls.push(CallSite {
        receipts: None,
        pretransfer_failure: None,
        forward_caller: true,
        callee: Callee::Operation(OperationRef(0)),
        operation_arguments: actuals,
        abi: FnAbiId(0),
        arguments,
        result,
        destination: abi.destination.then_some(DestinationClass::Inferred),
        declared: hirsel_lir::Effects::ALL,
        anchor: SourceAnchorId(0),
    });
    let next = b.builder.block(BlockKind::Body, &[]);
    let fail = b.builder.propagate();
    let normal = b.builder.edge(next, &[]);
    let failure = b.failure(fail);
    b.builder.term(
        ENTRY,
        Terminator::Call {
            call: CallId(0),
            normal: Some(normal),
            failure,
        },
    );
    b.builder.ret(next, result);
    let (unit, table, _) = b.finish();
    (unit, table)
}

fn check<S: Stage>(unit: &LirUnit<S>, table: &LayoutTable, registry: &OperationRegistry) -> Vec<K> {
    let manifest = Manifest::skeleton(registry.signatures.keys().cloned());
    hirsel_lir::validate::check(
        unit,
        &Inputs {
            memo: Default::default(),
            discharge: None,
            target: &TargetSpec::x86_64_unknown_linux_gnu(),
            layouts: table,
            manifest: &manifest,
            operations: registry,
        },
    )
    .into_iter()
    .map(|v| v.kind)
    .collect()
}

#[test]
fn net_read_cannot_use_the_physically_identical_read_into_abi() {
    let (registry, read, intrinsic) = network();
    let (mut unit, table) = fixture(&registry, &intrinsic, vec![A::Size(6)]);
    assert_eq!(check(&unit, &table, &registry), []);
    let (read_abi, _) = registry
        .derive(&read, &[], &TargetSpec::x86_64_unknown_linux_gnu())
        .unwrap();
    let intrinsic_abi = &table.fn_abis[0];
    assert_eq!(
        read_abi
            .parameters
            .iter()
            .map(|p| (&p.passing, &p.slots))
            .collect::<Vec<_>>(),
        intrinsic_abi
            .parameters
            .iter()
            .map(|p| (&p.passing, &p.slots))
            .collect::<Vec<_>>()
    );
    unit.tables.operations[0] = read;
    unit.tables.calls[0].operation_arguments.clear();
    assert_eq!(
        check(&unit, &table, &registry),
        [K::CallShape("operation declaration ABI")]
    );
}

#[test]
fn copying_the_declared_key_does_not_hide_wrong_parameter_layouts() {
    let (registry, read, intrinsic) = network();
    let (mut unit, mut table) = fixture(&registry, &intrinsic, vec![A::Size(6)]);
    assert_eq!(check(&unit, &table, &registry), []);
    let (expected, _) = registry
        .derive(&read, &[], &TargetSpec::x86_64_unknown_linux_gnu())
        .unwrap();
    table.fn_abis[0].key = expected.key;
    unit.tables.operations[0] = read;
    unit.tables.calls[0].operation_arguments.clear();
    assert_eq!(
        check(&unit, &table, &registry),
        [K::CallShape("operation declaration ABI")]
    );
}

#[test]
fn a_size_generic_operation_cannot_use_another_instances_abi() {
    let (registry, _, intrinsic) = network();
    let (mut unit, table) = fixture(&registry, &intrinsic, vec![A::Size(6)]);
    assert_eq!(check(&unit, &table, &registry), []);
    unit.tables.calls[0].operation_arguments = vec![A::Size(7)];
    assert_eq!(
        check(&unit, &table, &registry),
        [K::CallShape("operation declaration ABI")]
    );
}

#[test]
fn a_type_generic_operation_cannot_use_another_instances_abi() {
    let store = support::session();
    let t = ty(
        &store,
        TypeKind::Parameter(TypeParameterRef::bound(TypeParamId(0))),
    );
    let s = signature(
        "identity",
        &store,
        function(
            vec![parameter(t, PassingMode::Borrow)],
            t,
            vec![GenericParameter::Type {
                name: "T".into(),
                bounds: Default::default(),
            }],
        ),
    );
    let operation = id(&s);
    let registry = OperationRegistry {
        seal_sources: Default::default(),
        receipt_sources: Default::default(),
        pretransfer_adapters: Vec::new(),
        pretransfer_actuals: Vec::new(),
        invocation_bodies: Vec::new(),
        signatures: [(operation.clone(), s)].into(),
        ..support::registry()
    };
    let (mut unit, table) = fixture(
        &registry,
        &operation,
        vec![A::Type(NativeType::Primitive(
            hirsel_lir::operation::Primitive::I64,
        ))],
    );
    assert_eq!(check(&unit, &table, &registry), []);
    unit.tables.calls[0].operation_arguments = vec![A::Type(NativeType::Primitive(
        hirsel_lir::operation::Primitive::U64,
    ))];
    assert_eq!(
        check(&unit, &table, &registry),
        [K::CallShape("operation declaration ABI")]
    );
}

#[test]
fn concrete_actuals_round_trip_without_type_store_indices() {
    use hirsel_codec::{from_text, to_text};
    let types = vec![
        NativeType::Tuple(vec![NativeType::Primitive(
            hirsel_lir::operation::Primitive::U64,
        )]),
        NativeType::Record(vec![(
            false,
            NativeType::Primitive(hirsel_lir::operation::Primitive::Bytes),
        )]),
        NativeType::Variant(vec![
            vec![],
            vec![NativeType::Primitive(hirsel_lir::operation::Primitive::I32)],
        ]),
        NativeType::Box(Box::new(NativeType::Primitive(
            hirsel_lir::operation::Primitive::U8,
        ))),
        NativeType::Function(hirsel_lir::abi_types::CallMultiplicity::Once),
        NativeType::Named(
            path("Result"),
            vec![A::Type(NativeType::Primitive(
                hirsel_lir::operation::Primitive::U64,
            ))],
        ),
        NativeType::Builtin(
            hirsel_lir::operation::BuiltinType::Inline,
            vec![
                A::Type(NativeType::Primitive(hirsel_lir::operation::Primitive::U8)),
                A::Size(6),
            ],
        ),
    ];
    let arguments: Vec<_> = types.into_iter().map(A::Type).chain([A::Size(7)]).collect();
    assert_eq!(
        from_text::<Vec<A>>(&to_text(&arguments)).unwrap(),
        arguments
    );
    let (registry, _, intrinsic) = network();
    let (unit, table) = fixture(&registry, &intrinsic, vec![A::Size(6)]);
    let decoded: LirUnit<Value> = from_text(&to_text(&unit)).unwrap();
    assert_eq!(decoded, unit);
    assert_eq!(check(&decoded, &table, &registry), []);
}

#[test]
fn canonical_and_typed_scalar_children_share_the_declared_operation_abi() {
    let (registry, _, intrinsic) = network();
    let (mut unit, mut table) = fixture(&registry, &intrinsic, vec![A::Size(6)]);
    assert_eq!(check(&unit, &table, &registry), []);
    // LayoutBuilder::scalar and memory(Primitive) have distinct keys for the
    // same scalar. A cached String/ViewCursor can reach either leaf, depending
    // on which layout was interned first (Layouts::same_scalar).
    for record in &mut table.layouts {
        if matches!(record.kind, LayoutKind::Scalar(_)) {
            record.key.instance = Some(ContentHash::of_bytes(b"typed scalar key"));
        }
    }
    assert_eq!(check(&unit, &table, &registry), []);
}
