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
    let mut t = Ty::default();
    let private_shape = shape(&mut t);
    owners.nominals.insert(
        path(name),
        OwnerRepresentation::Defined(RepresentationRecord {
            definition: path(name),
            representation_revision: ContentHash::of_bytes(format!("{name}:{revision}").as_bytes()),
            generics: Vec::new(),
            types: t.store,
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
    let mut t = Ty::default();
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
    let mut t = Ty::default();
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
    let mut t = Ty::default();
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
        b.fn_abi(&t.store, many, CallableShape::CWrapper),
        Err(LayoutError::Unavailable { issue: Some(6), .. })
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
    let mut t = Ty::default();
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
    let s0 = t.store.push(qualified);
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

/// One signature `fn(borrow Id, take List[Id]) -> Id` in a store whose rows
/// are numbered differently, as an importer's public view would number them.
fn signature(padding: usize) -> (Ty, TypeId) {
    let mut t = Ty::default();
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
    let mut t = Ty::default();
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
fn direct_capture_parameters_have_no_hidden_environment_or_owned_result_loan() {
    let owners = Owners::default();
    let mut builder = LayoutBuilder::new(&x86(), &owners);
    let mut types = Ty::default();
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
