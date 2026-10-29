//! FnAbi derivation: passing classes from the declared convention,
//! hidden slots, relation erasure, defining/public agreement and validator
//! mutations.
use hirsel_core_contract::Passing;
use hirsel_layout::layout::{LayoutBuilder, OwnerRepresentation, Owners};
use hirsel_lir::abi::abi_digest;
use hirsel_lir::abi_types::*;
use hirsel_lir::layout::*;
use hirsel_lir::layout_types::*;
use hirsel_lir::layout_validate::{Obligation, validate_table};
use hirsel_syntax::{ContentHash, DefPath, ItemPath, PackageId, PassingMode};
use hirsel_tir::RepresentationRecord;
use hirsel_types::*;

fn path(name: &str) -> DefPath {
    DefPath(ItemPath {
        package: PackageId {
            name: "fixture".into(),
            version: "0".into(),
        },
        modules: vec!["m".into()],
        name: name.into(),
    })
}

use hirsel_types::testing::Types as Ty;

fn owner(
    owners: &mut Owners,
    name: &str,
    revision: &str,
    promises: Layout,
    shape: impl FnOnce(&mut Ty) -> TypeId,
) {
    let mut t = crate::support::ty();
    let private_shape = shape(&mut t);
    owners.nominals.insert(
        hirsel_syntax::DefId::intern(&path(name)),
        OwnerRepresentation::Defined(RepresentationRecord {
            definition: path(name),
            representation_revision: ContentHash::of_bytes(format!("{name}:{revision}").as_bytes()),
            generics: Vec::new(),
            private_shape,
            layout_promises: promises,
            resource: false,
            finalizer: None,
            unsafe_ffi_package: false,
        }),
    );
}

fn x86() -> TargetSpec {
    TargetSpec::x86_64_unknown_linux_gnu()
}

fn checked(b: LayoutBuilder<'_>) -> LayoutTable {
    let target = b.target().clone();
    let table = b.finish();
    if let Err(v) = validate_table(&table, &target) {
        panic!("derived table fails validation: {v:#?}");
    }
    table
}

const PTR: PhysicalSlot = PhysicalSlot {
    ty: PhysicalType::Pointer,
    size: 8,
    align: 8,
};
const I64: PhysicalSlot = PhysicalSlot {
    ty: PhysicalType::I64,
    size: 8,
    align: 8,
};

fn passing(abi: &FnAbi) -> Vec<(ParameterPassing, Vec<PhysicalSlot>, ScalarExtension)> {
    abi.parameters
        .iter()
        .map(|p| (p.passing, p.slots.clone(), p.extension))
        .collect()
}

#[test]
fn scalars_and_block_references_pass_by_value() {
    let owners = Owners::default();
    let mut b = LayoutBuilder::new(&x86(), &owners);
    let mut t = crate::support::ty();
    let (i8_, u16_, b_, i128_, f64_, s, unit) = (
        t.p(Primitive::I8),
        t.p(Primitive::U16),
        t.p(Primitive::Bool),
        t.p(Primitive::I128),
        t.p(Primitive::F64),
        t.p(Primitive::String),
        t.add(TypeKind::Tuple(Vec::new().into())),
    );
    let bx = t.add(TypeKind::Box(f64_));
    let callback = t.function(&[(PassingMode::Borrow, i8_)], unit, Multiplicity::Many, 0);
    let iter = t.add(TypeKind::Builtin {
        kind: BuiltinType::Iter,
        arguments: vec![GenericArgument::Type(i8_)].into(),
    });
    let f = t.function(
        &[
            (PassingMode::Borrow, i8_),
            (PassingMode::Take, u16_),
            (PassingMode::Borrow, b_),
            (PassingMode::Borrow, i128_),
            (PassingMode::Take, f64_),
            (PassingMode::Borrow, s),
            (PassingMode::Take, bx),
            (PassingMode::Borrow, callback),
            (PassingMode::Take, iter),
            (PassingMode::Borrow, unit),
        ],
        unit,
        Multiplicity::Many,
        0,
    );
    let id = b.fn_abi(&t.store, f, CallableShape::Definition).unwrap();
    let table = checked(b);
    let abi = &table.fn_abis[id.0 as usize];
    let slot = |ty, size, align| PhysicalSlot { ty, size, align };
    assert_eq!(
        passing(abi),
        [
            (
                ParameterPassing::ByValue,
                vec![slot(PhysicalType::I8, 1, 1)],
                ScalarExtension::Sign
            ),
            (
                ParameterPassing::ByValue,
                vec![slot(PhysicalType::I16, 2, 2)],
                ScalarExtension::Zero
            ),
            (
                ParameterPassing::ByValue,
                vec![slot(PhysicalType::I8, 1, 1)],
                ScalarExtension::Zero
            ),
            (
                ParameterPassing::ByValue,
                vec![slot(PhysicalType::I128, 16, 16)],
                ScalarExtension::None
            ),
            (
                ParameterPassing::ByValue,
                vec![slot(PhysicalType::F64, 8, 8)],
                ScalarExtension::None
            ),
            (
                ParameterPassing::ByValue,
                vec![PTR, I64, I64],
                ScalarExtension::None
            ),
            (ParameterPassing::ByValue, vec![PTR], ScalarExtension::None),
            (
                ParameterPassing::ByValue,
                vec![PTR, PTR],
                ScalarExtension::None
            ),
            (
                ParameterPassing::ByValue,
                vec![PTR, PTR, PTR],
                ScalarExtension::None
            ),
            (ParameterPassing::ByValue, vec![], ScalarExtension::None),
        ]
    );
    assert_eq!(abi.result, None);
    assert!(abi.context && !abi.environment && !abi.destination);
    assert_eq!(abi.version, hirsel_lir::ABI_VERSION);
}

#[test]
fn aggregates_and_nominals_pass_by_address_by_convention() {
    let mut owners = Owners::default();
    owner(&mut owners, "Meters", "", Layout::default(), |t| {
        t.p(Primitive::U64)
    });
    owner(
        &mut owners,
        "Packet",
        "",
        Layout {
            flat: true,
            ..Layout::default()
        },
        |t| {
            let (a, s) = (t.p(Primitive::U32), t.p(Primitive::String));
            t.record(&[a, s])
        },
    );
    let mut b = LayoutBuilder::new(&x86(), &owners);
    let mut t = crate::support::ty();
    let (i64_, s) = (t.p(Primitive::I64), t.p(Primitive::String));
    let pair = t.record(&[i64_, s]);
    let meters = t.named(&path("Meters"), Vec::new());
    let packet = t.named(&path("Packet"), Vec::new());
    let f = t.function(
        &[
            (PassingMode::Borrow, pair),
            (PassingMode::Take, pair),
            (PassingMode::Mut, pair),
            (PassingMode::Mut, i64_),
            (PassingMode::Borrow, meters),
            (PassingMode::Borrow, packet),
            (PassingMode::Take, packet),
        ],
        pair,
        Multiplicity::Many,
        0,
    );
    let scalar_result = t.function(&[], i64_, Multiplicity::Many, 0);
    let named_result = t.function(&[], meters, Multiplicity::Many, 0);
    let id = b.fn_abi(&t.store, f, CallableShape::Definition).unwrap();
    let sid = b
        .fn_abi(&t.store, scalar_result, CallableShape::Definition)
        .unwrap();
    let nid = b
        .fn_abi(&t.store, named_result, CallableShape::Definition)
        .unwrap();
    let table = checked(b);
    let abi = &table.fn_abis[id.0 as usize];
    let modes: Vec<_> = abi
        .parameters
        .iter()
        .map(|p| (p.convention, p.passing))
        .collect();
    assert_eq!(
        modes,
        [
            (Passing::Borrow, ParameterPassing::ReadOnlyAddress),
            (Passing::Take, ParameterPassing::TransferAddress),
            (Passing::Mut, ParameterPassing::ExclusiveAddress),
            (Passing::Mut, ParameterPassing::ExclusiveAddress),
            // A distinct scalar is a nominal: by address, whatever its shape.
            (Passing::Borrow, ParameterPassing::ReadOnlyAddress),
            (Passing::Borrow, ParameterPassing::PointerLength),
            (Passing::Take, ParameterPassing::TransferAddress),
        ]
    );
    assert_eq!(abi.parameters[5].slots, [PTR, I64]);
    // Every non-unit result travels through an output pointer.
    assert_eq!(abi.result.as_ref().unwrap().slots, [PTR]);
    assert!(
        abi.destination,
        "a result holding a String takes a caller destination"
    );
    let s = &table.fn_abis[sid.0 as usize];
    assert!(
        s.result.is_some() && !s.destination,
        "a scalar result allocates nothing"
    );
    assert!(
        table.fn_abis[nid.0 as usize].destination,
        "a nominal result is opaque"
    );
}

#[test]
fn thunks_take_an_environment_and_keep_multiplicity() {
    let owners = Owners::default();
    let mut b = LayoutBuilder::new(&x86(), &owners);
    let mut t = crate::support::ty();
    let (i64_, unit) = (
        t.p(Primitive::I64),
        t.add(TypeKind::Tuple(Vec::new().into())),
    );
    let many = t.function(&[(PassingMode::Borrow, i64_)], i64_, Multiplicity::Many, 0);
    let once = t.function(&[(PassingMode::Borrow, i64_)], i64_, Multiplicity::Once, 0);
    let generic = t.function(&[], unit, Multiplicity::Many, 1);
    let def = b.fn_abi(&t.store, many, CallableShape::Definition).unwrap();
    let clo = b.fn_abi(&t.store, many, CallableShape::Closure).unwrap();
    let onc = b.fn_abi(&t.store, once, CallableShape::Closure).unwrap();
    let next = b.fn_abi(&t.store, many, CallableShape::IterNext).unwrap();
    assert_eq!(
        b.fn_abi(&t.store, many, CallableShape::Closure).unwrap(),
        clo
    );
    assert!(matches!(
        b.fn_abi(&t.store, many, CallableShape::Foreign),
        Err(LayoutError::Invalid(_))
    ));
    assert_eq!(
        b.fn_abi(&t.store, generic, CallableShape::Definition),
        Err(LayoutError::Incomplete)
    );
    assert_eq!(
        b.fn_abi(&t.store, i64_, CallableShape::Definition),
        Err(LayoutError::Invalid("an ABI of a non-callable type".into()))
    );
    let table = checked(b);
    let get = |id: FnAbiId| &table.fn_abis[id.0 as usize];
    assert!(!get(def).environment);
    assert!(get(clo).environment && get(next).environment);
    assert_eq!(get(onc).multiplicity, CallMultiplicity::Once);
    assert_ne!(abi_digest(get(clo)), abi_digest(get(onc)));
    assert_ne!(abi_digest(get(def)), abi_digest(get(clo)));
}

#[test]
fn relations_never_change_an_fn_abi_but_conventions_do() {
    let owners = Owners::default();
    let mut b = LayoutBuilder::new(&x86(), &owners);
    let mut t = crate::support::ty();
    let rel = |i| Modes {
        relation: Relation::parameters([BinderRef::bound(ParamId(i))]),
        uniqueness: if i == 0 {
            Uniqueness::Unique
        } else {
            Uniqueness::Aliased
        },
        sendability: if i == 0 {
            Sendability::Sendable
        } else {
            Sendability::NotSendable
        },
        ..Modes::global()
    };
    let s0 = t.add_with(TypeKind::Primitive(Primitive::String), rel(0));
    let mut qualified = t.store.get(s0).unwrap().clone();
    qualified.sendability_requirement = hirsel_types::SendabilityRequirement::Sendable;
    let s0 = t.store.intern(qualified);
    let s1 = t.add_with(TypeKind::Primitive(Primitive::String), rel(1));
    let r0 = t.record(&[s0]);
    let r1 = t.record(&[s1]);
    let f0 = t.function(&[(PassingMode::Borrow, r0)], s0, Multiplicity::Many, 0);
    let f1 = t.function(&[(PassingMode::Borrow, r1)], s1, Multiplicity::Many, 0);
    let taken = t.function(&[(PassingMode::Take, r1)], s1, Multiplicity::Many, 0);
    let a = b.fn_abi(&t.store, f0, CallableShape::Definition).unwrap();
    assert_eq!(
        b.fn_abi(&t.store, f1, CallableShape::Definition).unwrap(),
        a
    );
    assert_eq!(
        b.layout(&t.store, r0).unwrap(),
        b.layout(&t.store, r1).unwrap()
    );
    let c = b
        .fn_abi(&t.store, taken, CallableShape::Definition)
        .unwrap();
    assert_ne!(a, c);
    let table = checked(b);
    assert_eq!(table.fn_abis.len(), 2);
}

#[test]
fn borrowed_cursor_results_preserve_their_declared_source_through_containers() {
    let mut owners = Owners::default();
    let cursor_path = path("BorrowedCursor");
    owners.nominals.insert(
        hirsel_syntax::DefId::intern(&cursor_path),
        OwnerRepresentation::Cursor {
            revision: ContentHash::of_bytes(b"borrowed cursor"),
        },
    );
    let mut types = crate::support::ty();
    let scalar = types.p(Primitive::U64);
    let cursor = types.named(&cursor_path, Vec::new());
    let pair = types.tuple(&[scalar, cursor]);
    let boxed = types.boxed(cursor);
    let mut builder = LayoutBuilder::new(&x86(), &owners);
    for result in [cursor, pair, boxed] {
        let mut result = types.store.get(result).unwrap().clone();
        result.modes.relation = Relation::parameters([BinderRef::bound(ParamId(0))]);
        let result = types.store.intern(result);
        let callable = types.function(
            &[(PassingMode::Borrow, scalar)],
            result,
            Multiplicity::Many,
            0,
        );
        let abi = builder
            .fn_abi(&types.store, callable, CallableShape::Definition)
            .unwrap();
        assert_eq!(
            builder.table().fn_abis[abi.0 as usize]
                .result
                .as_ref()
                .unwrap()
                .borrowed_parameters,
            [0],
            "result {result:?}",
        );
    }
    checked(builder);
}

/// One signature `fn(borrow Id, take List[Id]) -> Id` in a store whose rows
/// are numbered differently, as an importer's public view would number them.
fn signature(padding: usize) -> (Ty, TypeId) {
    let mut t = crate::support::ty();
    for _ in 0..padding {
        t.p(Primitive::Bool);
    }
    let id = t.named(&path("Id"), Vec::new());
    let list = t.add(TypeKind::Builtin {
        kind: BuiltinType::List,
        arguments: vec![GenericArgument::Type(id)].into(),
    });
    let f = t.function(
        &[(PassingMode::Borrow, id), (PassingMode::Take, list)],
        id,
        Multiplicity::Many,
        0,
    );
    (t, f)
}

fn owners_with_id(shape: &[Primitive]) -> Owners {
    let mut owners = Owners::default();
    owner(
        &mut owners,
        "Id",
        &format!("{shape:?}"),
        Layout::default(),
        |t| {
            let fields: Vec<_> = shape.iter().map(|&p| t.p(p)).collect();
            t.record(&fields)
        },
    );
    owners
}

#[test]
fn defining_and_public_views_agree_and_private_changes_move_only_layout_keys() {
    let derive = |owners: &Owners, padding| {
        let (t, f) = signature(padding);
        let mut b = LayoutBuilder::new(&x86(), owners);
        let id = b.fn_abi(&t.store, f, CallableShape::Definition).unwrap();
        let table = checked(b);
        let abi = table.fn_abis[id.0 as usize].clone();
        let key = table.layout(abi.parameters[0].layout).unwrap().key.clone();
        (abi_digest(&abi), key)
    };
    let narrow = owners_with_id(&[Primitive::U32]);
    let wide = owners_with_id(&[Primitive::U64, Primitive::String]);
    let defining = derive(&narrow, 0);
    let public = derive(&narrow, 7);
    assert_eq!(
        defining, public,
        "defining and public views produce identical ABI digests and layouts"
    );
    let changed = derive(&wide, 0);
    assert_eq!(
        changed.0, defining.0,
        "a private representation change keeps the ABI digest"
    );
    assert_ne!(changed.1, defining.1, "and moves the native layout key");
}

#[test]
fn the_validator_rejects_abi_mutations() {
    let owners = Owners::default();
    let mut b = LayoutBuilder::new(&x86(), &owners);
    let mut t = crate::support::ty();
    let (i32_, s) = (t.p(Primitive::I32), t.p(Primitive::String));
    let pair = t.record(&[i32_, s]);
    let f = t.function(
        &[
            (PassingMode::Borrow, i32_),
            (PassingMode::Borrow, pair),
            (PassingMode::Mut, s),
        ],
        s,
        Multiplicity::Many,
        0,
    );
    b.fn_abi(&t.store, f, CallableShape::Closure).unwrap();
    let table = checked(b);
    type Mutation = Box<dyn Fn(&mut FnAbi)>;
    let mutations: Vec<Mutation> = vec![
        Box::new(|a| a.parameters[1].passing = ParameterPassing::ByValue),
        Box::new(|a| a.parameters[0].slots = vec![I64]),
        Box::new(|a| a.parameters[0].extension = ScalarExtension::Zero),
        Box::new(|a| a.parameters[2].passing = ParameterPassing::ReadOnlyAddress),
        Box::new(|a| a.environment = false),
        Box::new(|a| a.context = false),
        Box::new(|a| a.result.as_mut().unwrap().slots = vec![PTR, PTR]),
        Box::new(|a| a.result.as_mut().unwrap().register = Some(ScalarKind::U64)),
        Box::new(|a| a.parameters.swap(0, 1)),
    ];
    for (i, mutate) in mutations.iter().enumerate() {
        let mut m = table.clone();
        mutate(&mut m.fn_abis[0]);
        let violations = validate_table(&m, &x86()).expect_err("mutation rejected");
        assert!(
            violations.iter().any(|v| v.obligation == Obligation::Abi),
            "mutation {i}: {violations:#?}"
        );
    }
}

#[test]
fn typed_register_results_agree_for_definitions_and_erased_entries() {
    for target in TargetSpec::executed() {
        let owners = Owners::default();
        let mut t = crate::support::ty();
        let mut b = LayoutBuilder::new(&target, &owners);
        let cases = [
            (Primitive::Bool, Some(ScalarKind::Bool)),
            (Primitive::I8, Some(ScalarKind::I8)),
            (Primitive::U64, Some(ScalarKind::U64)),
            (Primitive::F32, Some(ScalarKind::F32)),
            (Primitive::F64, Some(ScalarKind::F64)),
            (Primitive::I128, None),
            (Primitive::U128, None),
        ];
        let mut ids = Vec::new();
        for (primitive, expected) in cases {
            let result = t.p(primitive);
            let f = t.function(&[], result, Multiplicity::Many, 0);
            for shape in [CallableShape::Definition, CallableShape::Closure] {
                let id = b.fn_abi(&t.store, f, shape).unwrap();
                ids.push((id, expected));
            }
        }
        let table = checked(b);
        for (id, expected) in ids {
            assert_eq!(
                table.fn_abis[id.0 as usize]
                    .result
                    .as_ref()
                    .unwrap()
                    .register,
                expected
            );
            let mut forged = table.clone();
            forged.fn_abis[id.0 as usize]
                .result
                .as_mut()
                .unwrap()
                .register = if expected.is_some() {
                None
            } else {
                Some(ScalarKind::U64)
            };
            let violations =
                validate_table(&forged, &target).expect_err("forged result convention");
            assert!(violations.iter().any(|v| v.obligation == Obligation::Abi));
        }
    }
}

#[test]
fn direct_capture_parameters_have_no_hidden_environment_or_owned_result_loan() {
    let owners = Owners::default();
    let mut builder = LayoutBuilder::new(&x86(), &owners);
    let mut types = crate::support::ty();
    let scalar = types.p(Primitive::U64);
    let text = types.p(Primitive::String);
    let callable = types.function(
        &[(PassingMode::Borrow, scalar)],
        text,
        Multiplicity::Many,
        0,
    );
    let base = builder
        .fn_abi(&types.store, callable, CallableShape::Definition)
        .unwrap();
    let scalar = builder.layout(&types.store, scalar).unwrap();
    let text = builder.layout(&types.store, text).unwrap();
    let captures = [(scalar, Passing::Borrow), (text, Passing::Borrow)];
    let direct = builder
        .with_capture_parameters(base, &captures, true)
        .unwrap();
    assert_eq!(
        builder
            .with_capture_parameters(base, &captures, true)
            .unwrap(),
        direct
    );
    let table = checked(builder);
    let abi = &table.fn_abis[direct.0 as usize];
    assert_eq!(abi.shape, CallableShape::Definition);
    assert!(!abi.environment);
    assert_eq!(abi.parameters.len(), 3);
    assert_eq!(abi.parameters[1].layout, scalar);
    assert_eq!(abi.parameters[1].passing, ParameterPassing::ByValue);
    assert_eq!(abi.parameters[2].layout, text);
    assert_eq!(abi.parameters[2].passing, ParameterPassing::ReadOnlyAddress);
    assert!(abi.result.as_ref().unwrap().borrowed_parameters.is_empty());
}

#[test]
fn extern_abis_take_no_context_and_return_a_scalar_or_nothing() {
    let owners = Owners::default();
    let mut b = LayoutBuilder::new(&x86(), &owners);
    let mut t = crate::support::ty();
    let (i32_, s) = (t.p(Primitive::I32), t.p(Primitive::String));
    let plain = t.function(&[(PassingMode::Borrow, i32_)], i32_, Multiplicity::Many, 0);
    let text = t.function(&[], s, Multiplicity::Many, 0);
    let row = t.store.get(plain).unwrap().clone();
    let TypeKind::Function(mut function) = row.kind.clone() else {
        unreachable!()
    };
    function.foreign = Some(ForeignContract {
        abi: "c".into(),
        blocking: false,
        aliasing_contract: "Reads its argument only.".into(),
    });
    let foreign = t.add_with(TypeKind::Function(function), row.modes);
    // Only an extern has the foreign shape, and an extern has only it.
    assert!(matches!(
        b.fn_abi(&t.store, plain, CallableShape::Foreign),
        Err(LayoutError::Invalid(_))
    ));
    assert!(matches!(
        b.fn_abi(&t.store, foreign, CallableShape::Definition),
        Err(LayoutError::Invalid(_))
    ));
    let id = b.fn_abi(&t.store, foreign, CallableShape::Foreign).unwrap();
    b.fn_abi(&t.store, text, CallableShape::Definition).unwrap();
    let table = checked(b);
    let abi = &table.fn_abis[id.0 as usize];
    assert!(!abi.context && !abi.destination && !abi.environment);
    let aggregate = table
        .layouts
        .iter()
        .position(|l| !matches!(l.kind, LayoutKind::Scalar(_)))
        .map(|i| LayoutId(i as u32))
        .unwrap();
    // Controls: each mutated extern ABI is rejected only by the extern rules.
    type Mutation = Box<dyn Fn(&mut FnAbi)>;
    let mutations: Vec<(Mutation, &str)> = vec![
        (Box::new(|a| a.context = true), "no extern does"),
        (
            Box::new(|a| a.destination = true),
            "an extern returns a C scalar",
        ),
        (
            Box::new(move |a| a.result.as_mut().unwrap().layout = aggregate),
            "an extern returns a C scalar",
        ),
    ];
    for (i, (mutate, rule)) in mutations.iter().enumerate() {
        let mut m = table.clone();
        mutate(&mut m.fn_abis[id.0 as usize]);
        let violations = validate_table(&m, &x86()).expect_err("mutation rejected");
        assert!(
            violations
                .iter()
                .any(|v| v.obligation == Obligation::Abi && v.detail.contains(rule)),
            "mutation {i}: {violations:#?}"
        );
    }
}

/// Hand-built physical control: nullable pointer results are valid, but a
/// two-word record or nested option cannot use a C pointer's result register.
#[test]
fn foreign_results_admit_one_pointer_niche_without_admitting_aggregates() {
    for target in TargetSpec::executed() {
        let mut owners = Owners::default();
        owner(&mut owners, "Optional", "pointer", Layout::default(), |t| {
            let byte = t.p(Primitive::U8);
            let pointer = t.builtin(BuiltinType::Ptr, vec![GenericArgument::Type(byte)]);
            t.variant(&[&[], &[pointer]])
        });
        let mut b = LayoutBuilder::new(&target, &owners);
        let mut t = crate::support::ty();
        let byte = t.p(Primitive::U8);
        let pointer = t.builtin(BuiltinType::Ptr, vec![GenericArgument::Type(byte)]);
        let option = t.named(&path("Optional"), vec![]);
        let nested = t.variant(&[&[], &[option]]);
        let pair = t.record(&[pointer, pointer]);
        let nested = b.layout(&t.store, nested).unwrap();
        let pair = b.layout(&t.store, pair).unwrap();
        let plain = t.function(&[], option, Multiplicity::Many, 0);
        let row = t.store.get(plain).unwrap().clone();
        let TypeKind::Function(mut function) = row.kind else {
            panic!()
        };
        function.foreign = Some(ForeignContract {
            abi: "c".into(),
            blocking: false,
            aliasing_contract: "Returns NULL or a stable C pointer.".into(),
        });
        let foreign = t.add_with(TypeKind::Function(function), row.modes);
        let id = b.fn_abi(&t.store, foreign, CallableShape::Foreign).unwrap();
        let table = checked(b);
        for layout in [nested, pair] {
            let mut malformed = table.clone();
            malformed.fn_abis[id.0 as usize]
                .result
                .as_mut()
                .unwrap()
                .layout = layout;
            let rejected =
                validate_table(&malformed, &target).expect_err("a C result is one register");
            assert!(rejected.iter().any(|v| v.obligation == Obligation::Abi));
        }
    }
}

#[test]
fn callable_abis_retain_the_caller_site_carrier() {
    for shape in [
        CallableShape::Closure,
        CallableShape::IterNext,
        CallableShape::IterDrop,
    ] {
        let owners = Owners::default();
        let mut b = LayoutBuilder::new(&x86(), &owners);
        let mut t = crate::support::ty();
        let unit = t.add(TypeKind::Tuple(Vec::new().into()));
        let f = t.function(&[], unit, Multiplicity::Many, 0);
        let id = b.fn_abi(&t.store, f, shape).unwrap();
        let mut table = checked(b);
        assert!(table.fn_abis[id.0 as usize].caller_site);
        table.fn_abis[id.0 as usize].caller_site = false;
        assert!(
            validate_table(&table, &x86())
                .expect_err("missing carrier rejected")
                .iter()
                .any(|v| v.obligation == Obligation::Abi
                    && v.detail == "a callable ABI carries caller sites")
        );
    }
}

#[test]
fn foreign_abis_have_no_caller_site_carrier() {
    let owners = Owners::default();
    let mut b = LayoutBuilder::new(&x86(), &owners);
    let mut t = crate::support::ty();
    let unit = t.add(TypeKind::Tuple(Vec::new().into()));
    let plain = t.function(&[], unit, Multiplicity::Many, 0);
    let mut row = t.store.get(plain).unwrap().clone();
    let TypeKind::Function(ref mut function) = row.kind else {
        unreachable!()
    };
    function.foreign = Some(ForeignContract {
        abi: "c".into(),
        blocking: false,
        aliasing_contract: "No arguments or result.".into(),
    });
    let foreign = t.store.intern(row);
    let id = b.fn_abi(&t.store, foreign, CallableShape::Foreign).unwrap();
    assert!(b.with_caller_site(id).is_err());
    let mut table = checked(b);
    assert!(!table.fn_abis[id.0 as usize].caller_site);
    table.fn_abis[id.0 as usize].caller_site = true;
    assert!(
        validate_table(&table, &x86())
            .expect_err("foreign carrier rejected")
            .iter()
            .any(|v| v.obligation == Obligation::Abi
                && v.detail == "an extern has no caller sites")
    );
}

fn receipt_contract(
    mode: hirsel_lir::sealed_payload_source::ReceiptMode,
) -> hirsel_lir::sealed_payload_source::ReceiptContract {
    use hirsel_lir::sealed_payload_source::*;
    ReceiptContract::new(
        vec![ReceiptParameter {
            source: 0,
            ty: ReceiptType {
                source_type: ContentHash::of_bytes(b"original Bytes type"),
                profile: ContentHash::of_bytes(b"closed Bytes profile"),
            },
            mode,
        }],
        None,
        vec![],
        vec![],
    )
    .unwrap()
}

fn receipt_abi_fixture() -> (TargetSpec, LayoutTable, FnAbiId, FnAbiId) {
    use hirsel_lir::sealed_payload_source::ReceiptMode;
    let target = x86();
    let owners = Owners::default();
    let mut b = LayoutBuilder::new(&target, &owners);
    let mut t = crate::support::ty();
    let bytes = t.p(Primitive::Bytes);
    let unit = t.tuple(&[]);
    let callable = t.function(&[(PassingMode::Borrow, bytes)], unit, Multiplicity::Many, 0);
    let ordinary = b
        .fn_abi(&t.store, callable, CallableShape::Definition)
        .unwrap();
    let aware = b
        .receipt_fn_abi(ordinary, &receipt_contract(ReceiptMode::Borrow))
        .unwrap();
    (target, checked(b), ordinary, aware)
}

#[test]
fn receipt_absence_preserves_the_legacy_abi_key_encoding_and_hash() {
    use std::hash::{Hash, Hasher};
    let key = FnAbiKey {
        callable: ContentHash([0x11; 32]),
        abi_version: 7,
        receipt_contract: None,
    };
    let expected = format!("(FnAbiKey #{} 7)", "11".repeat(32));
    assert_eq!(hirsel_codec::to_text(&key), expected);
    let decoded: FnAbiKey = hirsel_codec::from_text(&expected).unwrap();
    assert_eq!(decoded, key);
    let mut old = std::hash::DefaultHasher::new();
    (key.callable, key.abi_version).hash(&mut old);
    let mut new = std::hash::DefaultHasher::new();
    key.hash(&mut new);
    assert_eq!(old.finish(), new.finish());
}

#[test]
fn receipt_complete_content_survives_portable_layout_rebinding() {
    let (_, table, ordinary, aware) = receipt_abi_fixture();
    let keys = hirsel_lir::LayoutKeys::new(&table);
    let ordinary_key = keys.fn_abi(&table.fn_abis[ordinary.0 as usize]);
    let receipt_key = keys.fn_abi(&table.fn_abis[aware.0 as usize]);
    assert_ne!(ordinary_key, receipt_key);
    let text = hirsel_codec::to_text(&table);
    let decoded: LayoutTable = hirsel_codec::from_text(&text).unwrap();
    let decoded_keys = hirsel_lir::LayoutKeys::new(&decoded);
    assert_eq!(
        decoded_keys.fn_abi(&decoded.fn_abis[aware.0 as usize]),
        receipt_key
    );
    let original = &table.fn_abis[ordinary.0 as usize];
    let receipt = &decoded.fn_abis[aware.0 as usize];
    assert_ne!(
        hirsel_lir::abi::scoped_entry_identity(original, &table),
        hirsel_lir::abi::scoped_entry_identity(receipt, &decoded)
    );
    assert_eq!(original.key.callable, receipt.key.callable);
    assert_eq!(original.parameters, receipt.parameters);
}

#[test]
fn receipt_layout_c_refuses_missing_forged_and_incompatible_contracts() {
    use hirsel_lir::sealed_payload_source::ReceiptMode;
    let (target, good, _, aware) = receipt_abi_fixture();
    for mutation in 0..6 {
        let mut bad = good.clone();
        let abi = &mut bad.fn_abis[aware.0 as usize];
        match mutation {
            0 => abi.receipts = None,
            1 => abi.key.receipt_contract = None,
            2 => abi.key.receipt_contract = Some(ContentHash::of_bytes(b"forged contract")),
            3 => {
                // A valid hash cannot authorize the wrong declared passing mode.
                let contract = receipt_contract(ReceiptMode::Take);
                abi.key.receipt_contract = Some(contract.content().unwrap());
                abi.receipts = Some(contract);
            }
            4 => {
                abi.receipts.as_mut().unwrap().source_demand =
                    ContentHash::of_bytes(b"forged semantic output")
            }
            5 => {
                abi.shape = CallableShape::Foreign;
                abi.context = false;
            }
            _ => unreachable!(),
        }
        let errors = validate_table(&bad, &target).unwrap_err();
        assert!(
            errors.iter().any(
                |v| v.subject == hirsel_lir::layout_validate::Subject::FnAbi(aware)
                    && v.obligation == Obligation::Abi
            ),
            "mutation {mutation}: {errors:?}"
        );
    }
}

#[test]
fn receipt_aware_entry_and_callee_are_refused_until_transport_is_checked() {
    use crate::support::{ENTRY, Lir};
    use hirsel_lir::sealed_payload_source::ReceiptContract;
    use hirsel_lir::stage::Value;
    use hirsel_lir::validate::ViolationKind;
    // Even an empty physical contract represents an independently demanded
    // context. Its declaration alone cannot authorize execution or erasure.
    for target in 0..3 {
        let mut t = Lir::<Value>::new(&[], None);
        let next = t.builder.block(hirsel_lir::ir::BlockKind::Body, &[]);
        let failure = t.builder.propagate();
        t.call(ENTRY, &[], None, &[], (next, &[]), failure);
        let called = t.builder.unit.tables.calls[0].abi;
        t.builder.ret(next, None);
        if target != 0 {
            let id = if target == 1 {
                t.builder.unit.header.abi
            } else {
                called
            };
            let contract = ReceiptContract::new(vec![], None, vec![], vec![]).unwrap();
            let abi = &mut t.table.fn_abis[id.0 as usize];
            abi.key.receipt_contract = Some(contract.content().unwrap());
            abi.receipts = Some(contract);
        }
        let errors = t.check();
        if target == 0 {
            assert!(errors.is_empty(), "{errors:?}");
        } else {
            assert!(
                errors.iter().any(
                    |e| matches!(e, ViolationKind::Schema(s) if s.contains("receipt ABI transport"))
                ),
                "{errors:?}"
            );
        }
    }
}
