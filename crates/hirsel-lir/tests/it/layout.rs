//! Layout derivation and its independent validator: every M1b
//! representation family, scan/drop order, dynamic extents, unavailable
//! attribute combinations, validator mutations, golden vectors and C probes.
use hirsel_codec as snapshot;
use hirsel_layout::layout::{
    Container, EnvironmentShape, LayoutBuilder, OwnerRepresentation, Owners,
};
use hirsel_lir::layout::*;
use hirsel_lir::layout_types::*;
use hirsel_lir::layout_validate::{Obligation, validate_table};
use hirsel_lir::runtime_abi::HEADER_SIZE;
use hirsel_syntax::{ContentHash, DefId, DefPath, ItemPath, PackageId, PassingMode};
use hirsel_tir::{BodyId, RepresentationRecord};
use hirsel_types::*;

// ----- fixtures ------------------------------------------------------------

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

use crate::support::ty;

#[derive(Default)]
struct OwnerSpec {
    generics: u32,
    promises: Layout,
    resource: bool,
    finalizer: bool,
    /// Distinguishes private representations of one name.
    revision: &'static str,
}

fn representation(
    name: &str,
    spec: OwnerSpec,
    shape: impl FnOnce(&mut Ty) -> TypeId,
) -> RepresentationRecord {
    let mut t = ty();
    let private_shape = shape(&mut t);
    RepresentationRecord {
        definition: path(name),
        representation_revision: ContentHash::of_bytes(
            format!("{name}:{}", spec.revision).as_bytes(),
        ),
        generics: (0..spec.generics)
            .map(|i| GenericParameter::Type {
                name: format!("T{i}"),
                bounds: Vec::new().into(),
            })
            .collect(),
        private_shape,
        layout_promises: spec.promises,
        resource: spec.resource,
        finalizer: spec
            .finalizer
            .then(|| (path(&format!("{name}.drop")), BodyId(0))),
        unsafe_ffi_package: false,
    }
}

fn owner(
    owners: &mut Owners,
    name: &str,
    spec: OwnerSpec,
    shape: impl FnOnce(&mut Ty) -> TypeId,
) -> DefPath {
    let record = representation(name, spec, shape);
    owners.nominals.insert(
        DefId::intern(&path(name)),
        OwnerRepresentation::Defined(record),
    );
    path(name)
}

fn x86() -> TargetSpec {
    TargetSpec::x86_64_unknown_linux_gnu()
}

/// Validate and return the table; every derived table must pass.
fn checked(b: LayoutBuilder<'_>) -> LayoutTable {
    let target = b.target().clone();
    let table = b.finish();
    if let Err(v) = validate_table(&table, &target) {
        panic!("derived table fails validation: {v:#?}");
    }
    table
}

fn rec(table: &LayoutTable, id: LayoutId) -> &LayoutRecord {
    table.layout(id).unwrap()
}

fn fixed(r: &LayoutRecord) -> u64 {
    match r.size {
        SizeSpec::Fixed(n) => n,
        _ => panic!("dynamic size"),
    }
}

#[test]
fn ordered_handle_metadata_keeps_families_and_recursive_obtainable_layouts() {
    let mut owners = Owners::default();
    let node = owner(&mut owners, "ReceiverNode", OwnerSpec::default(), |t| {
        let node = t.named(&path("ReceiverNode"), vec![]);
        let receiver = t.builtin(BuiltinType::Receiver, vec![GenericArgument::Type(node)]);
        t.record(&[receiver])
    });
    // Owner shapes and request types belong to the same compile session.
    let mut t = ty();
    let scalar = t.p(Primitive::U64);
    let atomic = t.builtin(BuiltinType::Atomic, vec![GenericArgument::Type(scalar)]);
    let receiver = t.builtin(BuiltinType::Receiver, vec![GenericArgument::Type(atomic)]);
    let task = t.builtin(BuiltinType::Task, vec![GenericArgument::Type(receiver)]);
    let mutex = t.builtin(BuiltinType::Mutex, vec![GenericArgument::Type(task)]);
    let captures = t.record(&[atomic, receiver, task, mutex]);
    let recursive = t.named(&node, vec![]);
    for target in TargetSpec::executed() {
        let mut builder = LayoutBuilder::new(&target, &owners);
        let captures = builder.layout(&t.store, captures).unwrap();
        let recursive = builder.layout(&t.store, recursive).unwrap();
        let table = checked(builder);
        let fields = &rec(&table, captures).fields;
        for (index, family, obtains) in [
            (0, HandleKind::Atomic, None),
            (1, HandleKind::Receiver, Some(fields[0].layout)),
            (2, HandleKind::Task, Some(fields[1].layout)),
        ] {
            assert!(rec(&table, captures).scan.iter().any(|step| matches!(step,
                ScanStep::Handle { kind, ty: Some(ty), obtains: actual, .. }
                if *kind == family && *ty == fields[index].layout && *actual == obtains)));
        }
        assert!(
            table
                .layouts
                .iter()
                .any(|r| r.scan.iter().any(|step| matches!(step,
            ScanStep::Handle { kind: HandleKind::Mutex, ty: Some(ty), obtains: Some(value), .. }
            if *ty == fields[3].layout && *value == fields[2].layout)))
        );
        assert!(rec(&table, recursive).scan.iter().any(|step| matches!(step,
            ScanStep::Handle { kind: HandleKind::Receiver, obtains: Some(value), .. }
            if *value == recursive)));
    }
}

#[test]
fn ordered_handle_layout_keys_follow_typed_and_obtainable_references() {
    let mut owners = Owners::default();
    let node = owner(&mut owners, "KeyReceiverNode", OwnerSpec::default(), |t| {
        let node = t.named(&path("KeyReceiverNode"), vec![]);
        let receiver = t.builtin(BuiltinType::Receiver, vec![GenericArgument::Type(node)]);
        t.record(&[receiver])
    });
    let mut t = ty();
    let scalar = t.p(Primitive::U64);
    let atomic = t.builtin(BuiltinType::Atomic, vec![GenericArgument::Type(scalar)]);
    let receiver = t.builtin(BuiltinType::Receiver, vec![GenericArgument::Type(atomic)]);
    let task = t.builtin(BuiltinType::Task, vec![GenericArgument::Type(receiver)]);
    let captures = t.record(&[atomic, receiver, task]);
    let recursive = t.named(&node, vec![]);
    let decoy = t.p(Primitive::U16);
    let derive = |insert: bool| {
        let mut builder = LayoutBuilder::new(&x86(), &owners);
        if insert {
            builder.layout(&t.store, decoy).unwrap();
        }
        let capture_id = builder.layout(&t.store, captures).unwrap();
        let recursive_id = builder.layout(&t.store, recursive).unwrap();
        let table = checked(builder);
        let keys = hirsel_lir::LayoutKeys::new(&table);
        let roots = [capture_id, recursive_id];
        for id in roots {
            assert_eq!(hirsel_lir::LayoutKeys::rooted(&table, id), keys.key(id));
        }
        (roots.map(|id| keys.key(id)), roots)
    };
    let (base, base_ids) = derive(false);
    let (shifted, shifted_ids) = derive(true);
    assert_ne!(base_ids, shifted_ids);
    assert_eq!(
        base, shifted,
        "handle scan references contain program-local ids"
    );
}

/// All owner shapes and requests must share the compile session's interner.
/// A separate `Ty::default()` can read unrelated rows with the same local ids.
fn receiver_graph(target: &TargetSpec, order: &[usize]) -> (LayoutTable, [LayoutId; 4], LayoutId) {
    let mut owners = Owners::default();
    for (name, payload) in [
        ("ReceiverSelf", Some("ReceiverSelf")),
        ("ReceiverLeft", Some("ReceiverRight")),
        ("ReceiverRight", Some("ReceiverLeft")),
        ("ReceiverAcyclic", None),
    ] {
        owner(&mut owners, name, OwnerSpec::default(), |t| {
            let payload = match payload {
                Some(name) => t.named(&path(name), vec![]),
                None => t.p(Primitive::U64),
            };
            let receiver = t.builtin(BuiltinType::Receiver, vec![GenericArgument::Type(payload)]);
            t.record(&[receiver])
        });
    }
    let mut t = ty();
    let requests = [
        "ReceiverSelf",
        "ReceiverLeft",
        "ReceiverRight",
        "ReceiverAcyclic",
    ]
    .map(|name| t.named(&path(name), vec![]));
    let mut builder = LayoutBuilder::new(target, &owners);
    let mut roots = [LayoutId(0); 4];
    for &i in order {
        roots[i] = builder.layout(&t.store, requests[i]).unwrap();
    }
    let scalar = t.p(Primitive::U64);
    let scalar = builder.layout(&t.store, scalar).unwrap();
    (checked(builder), roots, scalar)
}

#[test]
fn recursive_receiver_graphs_keep_obtains_keys_and_linked_operands_in_every_request_order() {
    use hirsel_lir::handle_scan::{Program, TYPED_HANDLE, WORDS};

    for target in TargetSpec::executed() {
        let (base, roots, _) = receiver_graph(&target, &[0, 1, 2, 3]);
        let keys = hirsel_lir::LayoutKeys::new(&base);
        let expected_keys = roots.map(|id| keys.key(id));
        for order in permutations(roots.len()) {
            let (table, roots, scalar) = receiver_graph(&target, &order);
            let keys = hirsel_lir::LayoutKeys::new(&table);
            assert_eq!(roots.map(|id| keys.key(id)), expected_keys);
            assert_eq!(canonical(&table), canonical(&base));
            for (root, payload) in roots
                .into_iter()
                .zip([roots[0], roots[2], roots[1], scalar])
            {
                assert_eq!(hirsel_lir::LayoutKeys::rooted(&table, root), keys.key(root));
                let receiver = rec(&table, root).fields[0].layout;
                let expected = vec![ScanStep::Handle {
                    offset: 0,
                    kind: HandleKind::Receiver,
                    ty: Some(receiver),
                    obtains: Some(payload),
                }];
                assert_eq!(
                    rec(&table, root).scan,
                    expected,
                    "order {order:?}, root {root:?}"
                );
                assert_eq!(rec(&table, receiver).scan, expected);
                // Linking must patch both recursive operands; a missing target
                // cannot certify a complete descriptor.
                let program = Program::lower(rec(&table, root));
                assert!(program.referenced().any(|id| id == payload));
                assert!(
                    program
                        .linked(|id| (id != payload).then_some(id.0 + 17))
                        .is_none()
                );
                let words = program.linked(|id| Some(id.0 + 17)).unwrap();
                assert_eq!(
                    &words[..5],
                    &[
                        TYPED_HANDLE,
                        0,
                        HandleKind::Receiver.code(),
                        u64::from(receiver.0 + 17),
                        u64::from(payload.0 + 17)
                    ]
                );
                assert_eq!(words.len() % WORDS, 0);
            }
        }
    }
}

#[test]
fn recursive_receiver_validator_rejects_missing_wrong_and_dangling_obtains() {
    for target in TargetSpec::executed() {
        let (table, roots, scalar) = receiver_graph(&target, &[0, 1, 2, 3]);
        for root in roots {
            let receiver = rec(&table, root).fields[0].layout;
            for (id, replacement, obligation) in [
                (root, None, Obligation::Scan),
                (receiver, None, Obligation::Scan),
                // A valid but incorrect target must fail independently of
                // reference-range checks, against the field's own scan.
                (
                    root,
                    Some(if root == roots[3] { roots[0] } else { scalar }),
                    Obligation::Scan,
                ),
                (root, Some(LayoutId(u32::MAX)), Obligation::Reference),
            ] {
                let mut mutated = table.clone();
                let ScanStep::Handle { obtains, .. } = &mut mutated.layouts[id.0 as usize].scan[0]
                else {
                    panic!("expected receiver scan")
                };
                assert_ne!(*obtains, replacement);
                *obtains = replacement;
                let violations = validate_table(&mutated, &target).expect_err("invalid obtains");
                assert!(
                    violations.iter().any(|v| v.obligation == obligation),
                    "{violations:#?}"
                );
            }
        }
    }
}

fn offsets(r: &LayoutRecord) -> Vec<u64> {
    r.fields.iter().map(|f| f.offset).collect()
}

/// Run a drop program over concrete bytes, independently of the derivation:
/// the ordered (address, action) events a runtime would perform.
fn drop_events(
    table: &LayoutTable,
    id: LayoutId,
    bytes: &[u8],
    base: u64,
    out: &mut Vec<(u64, &'static str)>,
) {
    let r = rec(table, id);
    run_drop(table, &r.drop, bytes, base, out);
}

fn read_u64(bytes: &[u8], at: u64) -> u64 {
    u64::from_le_bytes(bytes[at as usize..at as usize + 8].try_into().unwrap())
}

fn run_drop(
    table: &LayoutTable,
    steps: &[DropStep],
    bytes: &[u8],
    base: u64,
    out: &mut Vec<(u64, &'static str)>,
) {
    for step in steps {
        match step {
            DropStep::Finalize => out.push((base, "finalize")),
            DropStep::Release { offset } => out.push((base + offset, "release")),
            DropStep::ReleaseRoot { offset } => out.push((base + offset, "release root")),
            DropStep::ReleaseSealed { offset } => out.push((base + offset, "release sealed")),
            DropStep::ReleaseReceipt { offset } => out.push((base + offset, "release receipt")),
            DropStep::Environment { offset } => out.push((base + offset, "environment")),
            DropStep::CloseHandle { offset } => out.push((base + offset, "close")),
            DropStep::Child { offset, layout } => {
                drop_events(table, *layout, bytes, base + offset, out)
            }
            DropStep::Thaw { offset, .. } => out.push((base + offset, "thaw")),
            DropStep::ActiveVariant {
                tag_offset,
                form,
                variants,
            } => {
                let at = (base + tag_offset) as usize;
                let width = form.width(8) as usize;
                let mut word = [0u8; 8];
                word[..width].copy_from_slice(&bytes[at..at + width]);
                let tag = form.active(u64::from_le_bytes(word));
                if let Some((_, steps)) = variants.iter().find(|(o, _)| *o == tag) {
                    run_drop(table, steps, bytes, base, out);
                }
            }
            DropStep::ElementsReverse {
                count_offset,
                offset,
                stride,
                element,
            } => {
                let n = read_u64(bytes, base + count_offset);
                for i in (0..n).rev() {
                    drop_events(table, *element, bytes, base + offset + i * stride, out);
                }
            }
            DropStep::RepeatReverse {
                offset,
                count,
                stride,
                element,
            } => {
                for i in (0..*count).rev() {
                    drop_events(table, *element, bytes, base + offset + i * stride, out);
                }
            }
            DropStep::ColumnsReverse(c) => {
                let n = read_u64(bytes, base + c.count_offset);
                let cap = read_u64(bytes, base + c.capacity_offset);
                for i in (0..n).rev() {
                    // A variant row drops only the arm its tag column names.
                    let active = c.tag.map(|tag| {
                        let width = match tag.scalar {
                            ScalarKind::U8 => 1,
                            ScalarKind::U16 => 2,
                            _ => 4,
                        };
                        let at = (base + c.data_offset + cap * tag.prefix + i * width) as usize;
                        let mut word = [0u8; 4];
                        word[..width as usize].copy_from_slice(&bytes[at..at + width as usize]);
                        u32::from_le_bytes(word)
                    });
                    for column in c.columns.iter().rev() {
                        if active.is_some() && column.arm != active {
                            continue;
                        }
                        let at = base + c.data_offset + cap * column.prefix + i * column.stride;
                        drop_events(table, column.layout, bytes, at, out);
                    }
                }
            }
        }
    }
}

// ----- scalars and aggregates -------------------------------------------

#[test]
fn nested_mixed_inline_pointers_scan_and_drop_at_their_offsets() {
    let owners = Owners::default();
    let mut b = LayoutBuilder::new(&x86(), &owners);
    let mut t = ty();
    let (u8_, i64_, u16_, i32_, string) = (
        t.p(Primitive::U8),
        t.p(Primitive::I64),
        t.p(Primitive::U16),
        t.p(Primitive::I32),
        t.p(Primitive::String),
    );
    let boxed = t.boxed(i64_);
    let inner = t.record(&[u16_, string]);
    let outer = t.record(&[u8_, boxed, inner, i32_]);
    let id = b.layout(&t.store, outer).unwrap();
    let inner_id = b.layout(&t.store, inner).unwrap();
    let table = checked(b);
    let r = rec(&table, id);
    assert_eq!(offsets(r), [0, 8, 16, 48]);
    assert_eq!((fixed(r), r.align), (56, 8));
    assert_eq!(offsets(rec(&table, inner_id)), [0, 8]);
    assert_eq!(
        r.scan,
        [
            ScanStep::Counted { offset: 8 },
            ScanStep::SliceOwner { offset: 24 }
        ]
    );
    // Fields high to low: the inner String before the box.
    assert_eq!(
        r.drop,
        [
            DropStep::Release { offset: 24 },
            DropStep::Release { offset: 8 }
        ]
    );
    assert_eq!(r.copy, CopyRecipe::Fields);
    // The String wrapper (owner, start, length) is distinct from its bytes.
    let string_id = rec(&table, inner_id).fields[1].layout;
    let LayoutKind::Slice { element } = rec(&table, string_id).kind else {
        panic!("String is a slice wrapper")
    };
    assert_eq!(
        rec(&table, element).kind,
        LayoutKind::Scalar(ScalarKind::U8)
    );
    assert_eq!(fixed(rec(&table, string_id)), 24);
    let storage = table
        .layouts
        .iter()
        .find(|l| l.kind == LayoutKind::Sequence { element })
        .expect("String storage is a counted byte sequence");
    assert_eq!(storage.copy, CopyRecipe::NotCopyable);
}

#[test]
fn packed_fields_are_unaligned_and_align_raises_alignment() {
    let mut owners = Owners::default();
    let packed = owner(
        &mut owners,
        "Wire",
        OwnerSpec {
            promises: Layout {
                packed: true,
                ..Layout::default()
            },
            ..OwnerSpec::default()
        },
        |t| {
            let (a, b, c) = (t.p(Primitive::U8), t.p(Primitive::U64), t.p(Primitive::U16));
            let bx = t.boxed(b);
            t.record(&[a, b, c, bx])
        },
    );
    let aligned = owner(
        &mut owners,
        "Line",
        OwnerSpec {
            promises: Layout {
                align: Some(64),
                ..Layout::default()
            },
            ..OwnerSpec::default()
        },
        |t| {
            let a = t.p(Primitive::U8);
            t.record(&[a])
        },
    );
    let mut b = LayoutBuilder::new(&x86(), &owners);
    let mut t = ty();
    let p = t.named(&packed, vec![]);
    let a = t.named(&aligned, vec![]);
    let pid = b.layout(&t.store, p).unwrap();
    let aid = b.layout(&t.store, a).unwrap();
    let table = checked(b);
    let r = rec(&table, pid);
    assert_eq!(offsets(r), [0, 1, 9, 11]);
    assert_eq!((fixed(r), r.align), (19, 1));
    assert_eq!(
        r.fields.iter().map(|f| f.unaligned).collect::<Vec<_>>(),
        [false, true, true, true]
    );
    assert!(r.abi.unaligned);
    // An unaligned counted reference is still scanned and released at its offset.
    assert_eq!(r.scan, [ScanStep::Counted { offset: 11 }]);
    assert_eq!(r.drop, [DropStep::Release { offset: 11 }]);
    assert!(r.key.owner_revision.is_some());
    let r = rec(&table, aid);
    assert_eq!((fixed(r), r.align), (64, 64));
}

#[test]
fn variants_scan_and_drop_only_the_active_payload() {
    let owners = Owners::default();
    let mut b = LayoutBuilder::new(&x86(), &owners);
    let mut t = ty();
    let (u8_, i64_, string) = (
        t.p(Primitive::U8),
        t.p(Primitive::I64),
        t.p(Primitive::String),
    );
    let v = t.variant(&[&[u8_], &[i64_, string], &[]]);
    let id = b.layout(&t.store, v).unwrap();
    let table = checked(b);
    let r = rec(&table, id);
    assert_eq!((fixed(r), r.align), (40, 8));
    let tag = r.tag.as_ref().unwrap();
    assert_eq!(tag.offset, 0);
    assert_eq!(offsets_of(&tag.variants[0].fields), [8]);
    assert_eq!(offsets_of(&tag.variants[1].fields), [8, 16]);
    assert!(tag.variants[2].fields.is_empty());
    assert_eq!(
        r.scan,
        [ScanStep::ActiveVariant {
            tag_offset: 0,
            form: TagForm::Ordinal,
            variants: vec![(1, vec![ScanStep::SliceOwner { offset: 16 }])]
        }]
    );
    let mut bytes = vec![0u8; 40];
    for (tag, expected) in [(0u32, vec![]), (1, vec![(16, "release")]), (2, vec![])] {
        bytes[..4].copy_from_slice(&tag.to_le_bytes());
        let mut events = Vec::new();
        drop_events(&table, id, &bytes, 0, &mut events);
        assert_eq!(events, expected, "tag {tag}");
    }
}

/// `Leaf | Node(Box[Tree], Box[Tree])`, the binarytrees node, and `Node(i64, String,
/// Box[f64]) | Leaf`, whose niche word is the String's owner after the i64.
fn niche_table() -> (LayoutTable, LayoutId, LayoutId, LayoutId) {
    niche_table_for(&x86())
}

fn niche_table_for(target: &TargetSpec) -> (LayoutTable, LayoutId, LayoutId, LayoutId) {
    let mut owners = Owners::default();
    let tree = owner(&mut owners, "Tree", OwnerSpec::default(), |t| {
        let me = t.named(&path("Tree"), vec![]);
        let child = t.boxed(me);
        t.variant(&[&[], &[child, child]])
    });
    let mut b = LayoutBuilder::new(target, &owners);
    let mut t = ty();
    let (i64_, string, f64_) = (
        t.p(Primitive::I64),
        t.p(Primitive::String),
        t.p(Primitive::F64),
    );
    let bx = t.boxed(f64_);
    let shifted = t.variant(&[&[i64_, string, bx], &[]]);
    let named = t.named(&tree, vec![]);
    let boxed = t.boxed(named);
    let tid = b.layout(&t.store, named).unwrap();
    let bid = b.layout(&t.store, boxed).unwrap();
    let sid = b.layout(&t.store, shifted).unwrap();
    (checked(b), tid, bid, sid)
}

#[test]
fn a_fieldless_and_a_block_holding_variant_share_the_null_word_as_their_tag() {
    let (table, tid, bid, sid) = niche_table();
    let r = rec(&table, tid);
    assert_eq!((fixed(r), r.align), (16, 8));
    let tag = r.tag.as_ref().unwrap();
    assert_eq!(
        (tag.offset, tag.form),
        (
            0,
            TagForm::Niche {
                nullary: 0,
                dataful: 1
            }
        )
    );
    assert!(tag.variants[0].fields.is_empty());
    assert_eq!(offsets_of(&tag.variants[1].fields), [0, 8]);
    // No u32 tag leaf: the two words are the node's whole ABI.
    assert_eq!(
        r.abi.scalars,
        [(0, ScalarKind::Pointer), (8, ScalarKind::Pointer)]
    );
    assert_eq!(
        rec(&table, bid).kind,
        LayoutKind::Box { payload: tid },
        "a Box[Tree] block's payload is the 16-byte node"
    );
    let mut bytes = vec![0u8; 16];
    let mut events = Vec::new();
    drop_events(&table, tid, &bytes, 0, &mut events);
    assert_eq!(events, [], "a null word is a Leaf");
    bytes[..8].copy_from_slice(&0x7f00_0000_1000u64.to_le_bytes());
    drop_events(&table, tid, &bytes, 0, &mut events);
    assert_eq!(events, [(8, "release"), (0, "release")]);

    let s = rec(&table, sid);
    assert_eq!(fixed(s), 40);
    let tag = s.tag.as_ref().unwrap();
    assert_eq!(
        (tag.offset, tag.form),
        (
            8,
            TagForm::Niche {
                nullary: 1,
                dataful: 0
            }
        )
    );
}

#[test]
fn variants_without_one_fieldless_and_one_nonnull_word_holding_variant_keep_a_u32_tag() {
    let mut owners = Owners::default();
    let packed = owner(
        &mut owners,
        "PackedOpt",
        OwnerSpec {
            promises: Layout {
                packed: true,
                ..Layout::default()
            },
            ..OwnerSpec::default()
        },
        |t| {
            let f = t.p(Primitive::F64);
            let bx = t.boxed(f);
            t.variant(&[&[], &[bx]])
        },
    );
    let mut b = LayoutBuilder::new(&x86(), &owners);
    let mut t = ty();
    let (u8_, u64_, string, f64_) = (
        t.p(Primitive::U8),
        t.p(Primitive::U64),
        t.p(Primitive::String),
        t.p(Primitive::F64),
    );
    let bx = t.boxed(f64_);
    let maybe_text = t.variant(&[&[], &[string]]);
    let after_nullable = t.tuple(&[u64_, maybe_text]);
    let words = t.tuple(&[u64_, f64_]);
    let cases = [
        ("three variants", t.variant(&[&[], &[], &[bx]])),
        (
            "a nested variant's word may be null",
            t.variant(&[&[], &[maybe_text]]),
        ),
        (
            "a word only inside a nested variant",
            t.variant(&[&[], &[after_nullable]]),
        ),
        ("no pointer word", t.variant(&[&[], &[words]])),
        ("no fieldless variant", t.variant(&[&[u8_], &[bx]])),
        ("one variant", t.variant(&[&[bx]])),
        ("@packed", t.named(&packed, vec![])),
    ];
    let ids: Vec<_> = cases
        .iter()
        .map(|(_, ty)| b.layout(&t.store, *ty).unwrap())
        .collect();
    let table = checked(b);
    for ((name, _), id) in cases.iter().zip(ids) {
        let tag = rec(&table, id).tag.as_ref().unwrap();
        assert_eq!(tag.form, TagForm::Ordinal, "{name}");
    }
}

#[test]
fn the_validator_rejects_an_unsound_niche() {
    let (table, tid, _, sid) = niche_table();
    let t = tid.0 as usize;
    fn tag(m: &mut LayoutTable, at: usize) -> &mut TagEncoding {
        m.layouts[at].tag.as_mut().unwrap()
    }

    let mut m = table.clone();
    tag(&mut m, t).offset = 4; // no block reference word there
    rejects(&m, Obligation::Tag);

    let mut m = table.clone();
    let field = tag(&mut m, t).variants[1].fields[1].clone();
    tag(&mut m, t).variants[0].fields.push(field); // the null variant holds a field
    rejects(&m, Obligation::Tag);

    let mut m = table.clone();
    tag(&mut m, t).form = TagForm::Niche {
        nullary: 1,
        dataful: 1,
    };
    rejects(&m, Obligation::Tag);

    let mut m = table.clone();
    tag(&mut m, t).form = TagForm::Niche {
        nullary: 1,
        dataful: 0,
    }; // the word belongs to the fieldless variant
    rejects(&m, Obligation::Tag);

    let mut m = table.clone();
    tag(&mut m, sid.0 as usize).offset = 0; // the i64, not a block reference
    rejects(&m, Obligation::Tag);

    let mut m = table.clone();
    m.layouts[t].kind = LayoutKind::Packed;
    rejects(&m, Obligation::Tag);

    let mut m = table.clone();
    tag(&mut m, t).form = TagForm::Ordinal; // a u32 tag over the left child
    rejects(&m, Obligation::Tag);

    let mut m = table.clone();
    let DropStep::ActiveVariant { form, .. } = &mut m.layouts[t].drop[0] else {
        panic!()
    };
    *form = TagForm::Ordinal; // the drop program reads another encoding
    rejects(&m, Obligation::DropOrder);
}

/// `None | Some(T)` over payloads whose first non-null word is nested: a String in a
/// tuple (the map entry `(String, u64)`) and after a u64, a String in a record after a
/// u8, a String two tuples deep, and a Box after a nullable `Option[Box]`.
fn nested_niche_table() -> (LayoutTable, [LayoutId; 5]) {
    let owners = Owners::default();
    let mut b = LayoutBuilder::new(&x86(), &owners);
    let mut t = ty();
    let (u8_, u32_, u64_, string, f64_) = (
        t.p(Primitive::U8),
        t.p(Primitive::U32),
        t.p(Primitive::U64),
        t.p(Primitive::String),
        t.p(Primitive::F64),
    );
    let bx = t.boxed(f64_);
    let maybe_box = t.variant(&[&[], &[bx]]);
    let entry = t.tuple(&[string, u64_]);
    let late = t.tuple(&[u64_, string]);
    let record = t.record(&[u8_, string]);
    let inner = t.tuple(&[u32_, string]);
    let deep = t.tuple(&[u64_, inner]);
    let guarded = t.tuple(&[maybe_box, bx]);
    let ids = [entry, late, record, deep, guarded].map(|payload| {
        let option = t.variant(&[&[], &[payload]]);
        b.layout(&t.store, option).unwrap()
    });
    (checked(b), ids)
}

#[test]
fn a_nonnull_word_nested_in_a_string_tuple_or_record_is_the_niche() {
    let (table, ids) = nested_niche_table();
    // (size, niche offset): no variant grows past its payload.
    let expected = [(32, 0), (32, 8), (32, 8), (40, 16), (16, 8)];
    for (id, (size, offset)) in ids.into_iter().zip(expected) {
        let r = rec(&table, id);
        assert_eq!((fixed(r), r.align), (size, 8), "{offset}");
        let tag = r.tag.as_ref().unwrap();
        assert_eq!(
            (tag.offset, tag.form),
            (
                offset,
                TagForm::Niche {
                    nullary: 0,
                    dataful: 1
                }
            )
        );
        assert_eq!(offsets_of(&tag.variants[1].fields), [0]);
        let mut bytes = vec![0u8; size as usize];
        let mut events = Vec::new();
        drop_events(&table, id, &bytes, 0, &mut events);
        assert_eq!(events, [], "a null word is None");
        let at = offset as usize;
        bytes[at..at + 8].copy_from_slice(&0x7f00_0000_1000u64.to_le_bytes());
        drop_events(&table, id, &bytes, 0, &mut events);
        assert!(events.contains(&(offset, "release")), "{events:?}");
    }
}

#[test]
fn the_validator_rejects_a_nested_niche_word_that_may_be_null_uninitialized_or_misaligned() {
    let (table, [entry, _, record, deep, guarded]) = nested_niche_table();
    let owners = Owners::default();
    let mut b = LayoutBuilder::new(&x86(), &owners);
    let mut t = ty();
    let (u8_, i128_, string) = (
        t.p(Primitive::U8),
        t.p(Primitive::I128),
        t.p(Primitive::String),
    );
    let padded = t.tuple(&[u8_, i128_, string]);
    let option = t.variant(&[&[], &[padded]]);
    let pid = b.layout(&t.store, option).unwrap();
    let padded_table = checked(b);
    fn tag(m: &mut LayoutTable, id: LayoutId) -> &mut TagEncoding {
        m.layouts[id.0 as usize].tag.as_mut().unwrap()
    }
    fn moved(table: &LayoutTable, id: LayoutId, offset: u64) -> LayoutTable {
        let mut m = table.clone();
        tag(&mut m, id).offset = offset;
        m
    }

    // Nullable: the entry's u64, a String's length word, the u32 beside a nested
    // String, and the word of a nested `Option[Box]`.
    rejects(&moved(&table, entry, 24), Obligation::Tag);
    rejects(&moved(&table, entry, 8), Obligation::Tag);
    rejects(&moved(&table, deep, 8), Obligation::Tag);
    rejects(&moved(&table, guarded, 0), Obligation::Tag);
    // Uninitialized: aligned padding between the u8 and the i128.
    let p = rec(&padded_table, pid);
    let tag_at = p.tag.as_ref().unwrap();
    let tuple = rec(&padded_table, tag_at.variants[1].fields[0].layout);
    assert_eq!((tag_at.offset, offsets(tuple)), (32, vec![0, 16, 32]));
    rejects(&moved(&padded_table, pid, 8), Obligation::Tag);
    // Misaligned: inside the record's String owner, and through an unaligned field.
    rejects(&moved(&table, record, 12), Obligation::Tag);
    let mut m = table.clone();
    tag(&mut m, record).variants[1].fields[0].unaligned = true;
    rejects(&m, Obligation::Tag);
    let mut m = table.clone();
    let r = tag(&mut m, record).variants[1].fields[0].layout.0 as usize;
    let string = &mut m.layouts[r].fields[1];
    assert_eq!(string.offset, 8);
    string.unaligned = true;
    rejects(&m, Obligation::Tag);
}

#[test]
fn a_niche_lowers_to_one_nonnull_step_around_the_dataful_variant() {
    use hirsel_lir::rt_program::{self, DescriptorError, StepKind};
    let (table, tid, _, _) = niche_table();
    let d = rt_program::descriptor(&table, tid).unwrap();
    let steps: Vec<_> = d
        .steps
        .iter()
        .map(|s| (s.kind, s.field, s.span, s.offset))
        .collect();
    assert_eq!(
        steps,
        [
            (StepKind::NonNull, u32::MAX, 2, 0),
            (StepKind::Release, 1, 0, 8),
            (StepKind::ReleaseLast, 0, 0, 0),
        ]
    );
    let mut m = table.clone();
    let DropStep::ActiveVariant { variants, .. } = &mut m.layouts[tid.0 as usize].drop[0] else {
        panic!()
    };
    variants[0].0 = 0; // the null variant's arm
    assert!(matches!(
        rt_program::descriptor(&m, tid),
        Err(DescriptorError::Malformed(_))
    ));
}

fn offsets_of(fields: &[FieldLayout]) -> Vec<u64> {
    fields.iter().map(|f| f.offset).collect()
}

#[test]
fn resources_finalize_before_their_fields_high_to_low() {
    let mut owners = Owners::default();
    let res = owner(
        &mut owners,
        "File",
        OwnerSpec {
            resource: true,
            finalizer: true,
            ..OwnerSpec::default()
        },
        |t| {
            let (s, i) = (t.p(Primitive::String), t.p(Primitive::I64));
            let bx = t.boxed(i);
            t.record(&[s, bx])
        },
    );
    let bare = owner(
        &mut owners,
        "Token",
        OwnerSpec {
            resource: true,
            ..OwnerSpec::default()
        },
        |t| t.p(Primitive::U64),
    );
    let mut b = LayoutBuilder::new(&x86(), &owners);
    let mut t = ty();
    let r1 = t.named(&res, vec![]);
    let tok = t.named(&bare, vec![]);
    let pair = t.record(&[r1, tok, r1]);
    let rid = b.layout(&t.store, r1).unwrap();
    let pid = b.layout(&t.store, pair).unwrap();
    let tid = b.layout(&t.store, tok).unwrap();
    let table = checked(b);
    let r = rec(&table, rid);
    assert_eq!(r.kind, LayoutKind::Resource);
    assert_eq!(r.copy, CopyRecipe::NotCopyable);
    assert_eq!(r.finalizer.as_ref().unwrap().definition, path("File.drop"));
    assert_eq!(
        r.drop,
        [
            DropStep::Finalize,
            DropStep::Release { offset: 24 },
            DropStep::Release { offset: 0 }
        ]
    );
    assert!(!r.storage.contains(&StorageKind::Static));
    // A resource without a drop body is still a noncopyable resource.
    let tok_r = rec(&table, tid);
    assert_eq!(
        (tok_r.kind.clone(), tok_r.copy),
        (LayoutKind::Resource, CopyRecipe::NotCopyable)
    );
    assert!(tok_r.finalizer.is_none());
    let p = rec(&table, pid);
    assert_eq!(offsets(p), [0, 32, 40]);
    let mut events = Vec::new();
    drop_events(&table, pid, &[0u8; 72], 0, &mut events);
    assert_eq!(
        events,
        [
            (40, "finalize"),
            (64, "release"),
            (40, "release"),
            (0, "finalize"),
            (24, "release"),
            (0, "release")
        ]
    );
}

#[test]
fn soa_sequences_drop_elements_high_to_low_and_fields_in_reverse() {
    let mut owners = Owners::default();
    let particle = owner(
        &mut owners,
        "Particle",
        OwnerSpec {
            promises: Layout {
                soa: true,
                ..Layout::default()
            },
            ..OwnerSpec::default()
        },
        |t| {
            let (id, name, f) = (
                t.p(Primitive::U32),
                t.p(Primitive::String),
                t.p(Primitive::F64),
            );
            let bx = t.boxed(f);
            t.record(&[id, name, f, bx])
        },
    );
    let mut b = LayoutBuilder::new(&x86(), &owners);
    let mut t = ty();
    let p = t.named(&particle, vec![]);
    let list = t.list(p);
    let standalone = b.layout(&t.store, p).unwrap();
    let lid = b.layout(&t.store, list).unwrap();
    let table = checked(b);
    // Standalone records stay inline in declaration order.
    assert_eq!(offsets(rec(&table, standalone)), [0, 8, 32, 40]);
    let LayoutKind::Block { payload } = rec(&table, lid).kind else {
        panic!("a list is a block")
    };
    let payload_r = rec(&table, payload);
    assert_eq!(
        payload_r.kind,
        LayoutKind::SoaColumns {
            element: standalone
        }
    );
    let DropStep::ColumnsReverse(columns) = &payload_r.drop[0] else {
        panic!("SoA drop")
    };
    assert_eq!(columns.data_offset, 16);
    assert_eq!(columns.capacity_quantum, 8);
    assert_eq!(
        columns
            .columns
            .iter()
            .map(|c| (c.field, c.prefix, c.stride))
            .collect::<Vec<_>>(),
        [(1, 4, 24), (3, 36, 8)]
    );
    // Two live elements in capacity 2: column bases 16 + 2 * prefix.
    let mut bytes = vec![0u8; 16 + 2 * 44];
    bytes[..8].copy_from_slice(&2u64.to_le_bytes());
    bytes[8..16].copy_from_slice(&2u64.to_le_bytes());
    assert_eq!(extent(&table, payload, &bytes), Ok(16 + 2 * 44));
    let mut events = Vec::new();
    drop_events(&table, payload, &bytes, 0, &mut events);
    let name = |i: u64| 16 + 2 * 4 + i * 24;
    let boxed = |i: u64| 16 + 2 * 36 + i * 8;
    assert_eq!(
        events,
        [
            (boxed(1), "release"),
            (name(1), "release"),
            (boxed(0), "release"),
            (name(0), "release")
        ]
    );
}

#[test]
fn soa_variants_store_a_narrow_tag_column_and_drop_only_active_payloads() {
    let mut owners = Owners::default();
    let soa = || OwnerSpec {
        promises: Layout {
            soa: true,
            ..Layout::default()
        },
        ..OwnerSpec::default()
    };
    let named = owner(&mut owners, "Named", soa(), |t| {
        let string = t.p(Primitive::String);
        t.variant(&[&[], &[string]])
    });
    let sample = owner(&mut owners, "Sample", soa(), |t| {
        let (u8_, i64_, u128_, string) = (
            t.p(Primitive::U8),
            t.p(Primitive::I64),
            t.p(Primitive::U128),
            t.p(Primitive::String),
        );
        t.variant(&[&[u8_], &[string, i64_], &[u128_], &[]])
    });
    let mut b = LayoutBuilder::new(&x86(), &owners);
    let mut t = ty();
    let n = t.named(&named, vec![]);
    let s = t.named(&sample, vec![]);
    let list = t.list(s);
    let named_row = b.layout(&t.store, n).unwrap();
    let row = b.layout(&t.store, s).unwrap();
    let lid = b.layout(&t.store, list).unwrap();
    let table = checked(b);
    // An @soa variant has no null-pointer niche: its rows keep an ordinal tag.
    assert_eq!(
        rec(&table, named_row)
            .tag
            .as_ref()
            .map(|t| (t.offset, t.form)),
        Some((0, TagForm::Ordinal))
    );
    // Row: u32 tag, payload from 16 (u128 alignment), 48 bytes in all.
    assert_eq!((fixed(rec(&table, row)), rec(&table, row).align), (48, 16));
    let LayoutKind::Block { payload } = rec(&table, lid).kind else {
        panic!("a list is a block")
    };
    let DropStep::ColumnsReverse(columns) = &rec(&table, payload).drop[0] else {
        panic!("SoA drop")
    };
    // Four arms fit a u8 tag column; the payload column follows it, 32 bytes a row.
    assert_eq!(
        columns.tag,
        Some(SoaTag {
            prefix: 0,
            scalar: ScalarKind::U8
        })
    );
    assert_eq!((columns.data_offset, columns.capacity_quantum), (16, 16));
    assert_eq!(
        columns
            .columns
            .iter()
            .map(|c| (c.field, c.prefix, c.stride, c.arm))
            .collect::<Vec<_>>(),
        [(1, 1, 32, Some(1))]
    );
    // Capacity 16, two rows: row 0 is the String arm, row 1 the u8 arm.
    let mut bytes = vec![0u8; 16 + 16 * 33];
    bytes[..8].copy_from_slice(&2u64.to_le_bytes());
    bytes[8..16].copy_from_slice(&16u64.to_le_bytes());
    bytes[16] = 1;
    bytes[17] = 0;
    assert_eq!(extent(&table, payload, &bytes), Ok(16 + 16 * 33));
    let mut events = Vec::new();
    drop_events(&table, payload, &bytes, 0, &mut events);
    assert_eq!(events, [(16 + 16, "release")]);

    // V: an arm view must sit under the tag column, as its own arm.
    let target = x86();
    let mutate = |edit: &dyn Fn(&mut SoaColumns)| {
        let mut bad = table.clone();
        let record = &mut bad.layouts[payload.0 as usize];
        let DropStep::ColumnsReverse(c) = &mut record.drop[0] else {
            unreachable!()
        };
        edit(c);
        validate_table(&bad, &target).is_err()
    };
    assert!(mutate(&|c| c.columns[0].arm = None));
    assert!(mutate(&|c| c.columns[0].arm = Some(0)));
    assert!(mutate(&|c| c.tag = None));
    assert!(mutate(&|c| c.columns.clear()));
}

#[test]
fn empty_large_and_overflowing_inline_arrays() {
    let owners = Owners::default();
    let mut b = LayoutBuilder::new(&x86(), &owners);
    let mut t = ty();
    let (u8_, i64_, string) = (
        t.p(Primitive::U8),
        t.p(Primitive::I64),
        t.p(Primitive::String),
    );
    let empty = t.inline(string, 0);
    let large = t.inline(i64_, 1 << 20);
    let strings = t.inline(string, 3);
    let bytes4 = t.inline(u8_, 4);
    let huge = t.inline(i64_, 1 << 62);
    let overflow = t.inline(huge, 8);
    let e = b.layout(&t.store, empty).unwrap();
    let l = b.layout(&t.store, large).unwrap();
    let s = b.layout(&t.store, strings).unwrap();
    let w = b.layout(&t.store, bytes4).unwrap();
    assert_eq!(b.layout(&t.store, overflow), Err(LayoutError::Overflow));
    let table = checked(b);
    assert_eq!((fixed(rec(&table, e)), rec(&table, e).align), (0, 8));
    assert!(rec(&table, e).drop.is_empty());
    assert_eq!(fixed(rec(&table, l)), 8 << 20);
    assert!(
        rec(&table, l).abi.scalars.is_empty(),
        "too large for register classification"
    );
    assert_eq!(rec(&table, w).abi.scalars.len(), 4);
    let mut events = Vec::new();
    drop_events(&table, s, &[0; 72], 0, &mut events);
    assert_eq!(events, [(48, "release"), (24, "release"), (0, "release")]);
}

#[test]
fn simd_requires_positive_lanes_and_aligns_to_the_vector_width() {
    let owners = Owners::default();
    let mut b = LayoutBuilder::new(&x86(), &owners);
    let mut t = ty();
    let f = t.p(Primitive::F32);
    let simd = |t: &mut Ty, n| {
        t.builtin(
            BuiltinType::Simd,
            vec![
                GenericArgument::Type(f),
                GenericArgument::Size(Size::Constant(n)),
            ],
        )
    };
    let (v3, v8, v0) = (simd(&mut t, 3), simd(&mut t, 8), simd(&mut t, 0));
    let a = b.layout(&t.store, v3).unwrap();
    let c = b.layout(&t.store, v8).unwrap();
    assert!(matches!(
        b.layout(&t.store, v0),
        Err(LayoutError::Invalid(_))
    ));
    let table = checked(b);
    assert_eq!((fixed(rec(&table, a)), rec(&table, a).align), (16, 16));
    assert_eq!((fixed(rec(&table, c)), rec(&table, c).align), (32, 16));
}

#[test]
fn simd_admits_bool_lanes_of_one_byte_and_refuses_a_non_scalar_lane() {
    let owners = Owners::default();
    let mut b = LayoutBuilder::new(&x86(), &owners);
    let mut t = ty();
    let (bool_, string) = (t.p(Primitive::Bool), t.p(Primitive::String));
    let simd = |t: &mut Ty, lane| {
        t.builtin(
            BuiltinType::Simd,
            vec![
                GenericArgument::Type(lane),
                GenericArgument::Size(Size::Constant(4)),
            ],
        )
    };
    let (mask, text) = (simd(&mut t, bool_), simd(&mut t, string));
    let id = b.layout(&t.store, mask).unwrap();
    assert!(matches!(
        b.layout(&t.store, text),
        Err(LayoutError::Invalid(_))
    ));
    let table = checked(b);
    assert_eq!((fixed(rec(&table, id)), rec(&table, id).align), (4, 4));
    assert_eq!(rec(&table, id).copy, CopyRecipe::Bits);
}

// ----- dynamic extents ----------------------------------------------------

#[test]
fn packed_variants_take_only_the_active_variant_extent() {
    let owners = Owners::default();
    let mut b = LayoutBuilder::new(&x86(), &owners);
    let mut t = ty();
    let (u8_, i64_) = (t.p(Primitive::U8), t.p(Primitive::I64));
    let v = t.variant(&[&[u8_], &[i64_, i64_], &[]]);
    let packed = t.builtin(BuiltinType::Packed, vec![GenericArgument::Type(v)]);
    let boxed = t.boxed(packed);
    let ordinary = b.layout(&t.store, v).unwrap();
    let id = b.layout(&t.store, packed).unwrap();
    let bid = b.layout(&t.store, boxed).unwrap();
    // Bounded Packed storage reserves its largest active extent inline.
    let holder = t.record(&[packed]);
    let hid = b.layout(&t.store, holder).unwrap();
    let table = checked(b);
    assert_eq!(fixed(rec(&table, hid)), 24);
    assert_eq!(fixed(rec(&table, ordinary)), 24);
    let r = rec(&table, id);
    assert_eq!(r.kind, LayoutKind::Packed);
    for (tag, size) in [(0u32, 16u64), (1, 24), (2, 8)] {
        let mut bytes = [0u8; 8];
        bytes[..4].copy_from_slice(&tag.to_le_bytes());
        assert_eq!(extent(&table, id, &bytes), Ok(size), "tag {tag}");
    }
    assert!(matches!(
        extent(&table, id, &[9, 0, 0, 0]),
        Err(LayoutError::Extent(_))
    ));
    // Packed behind a block: the payload keeps its dynamic extent.
    assert_eq!(rec(&table, bid).kind, LayoutKind::Box { payload: id });
}

/// A flat message: id u32, name String, a Packed status, then the growable
/// List[String] of tags. Sizes come from the bytes with checked arithmetic.
fn flat_message(owners: &mut Owners) -> DefPath {
    let status = owner(owners, "Status", OwnerSpec::default(), |t| {
        let (u16_, u64_) = (t.p(Primitive::U16), t.p(Primitive::U64));
        t.variant(&[&[u16_], &[u64_, u16_]])
    });
    owner(
        owners,
        "Message",
        OwnerSpec {
            promises: Layout {
                flat: true,
                ..Layout::default()
            },
            ..OwnerSpec::default()
        },
        |t| {
            let (id, name) = (t.p(Primitive::U32), t.p(Primitive::String));
            let st = t.named(&status, vec![]);
            let packed = t.builtin(BuiltinType::Packed, vec![GenericArgument::Type(st)]);
            let tags = t.list(name);
            t.growable_record(&[id, name, packed, tags])
        },
    )
}

/// FlatFormatV0 golden bytes for one Message (little-endian, no padding).
fn message_bytes() -> Vec<u8> {
    let mut v = Vec::new();
    v.extend(7u32.to_le_bytes()); // id
    v.extend(3u64.to_le_bytes()); // name length
    v.extend(b"abc");
    v.extend(14u64.to_le_bytes()); // Packed status length
    v.extend(1u32.to_le_bytes()); // ordinal 1: u64, u16
    v.extend(9u64.to_le_bytes());
    v.extend(2u16.to_le_bytes());
    v.extend(29u64.to_le_bytes()); // tags length
    v.extend(2u64.to_le_bytes()); // tag count
    v.extend(1u64.to_le_bytes());
    v.extend(b"x");
    v.extend(4u64.to_le_bytes());
    v.extend(b"yzzy");
    v
}

#[test]
fn flat_v0_extents_follow_the_sequential_cursor() {
    let mut owners = Owners::default();
    let message = flat_message(&mut owners);
    let mut b = LayoutBuilder::new(&x86(), &owners);
    let mut t = ty();
    let m = t.named(&message, vec![]);
    let body = b.flat(&t.store, m).unwrap();
    let value = b.layout(&t.store, m).unwrap();
    let table = checked(b);
    let r = rec(&table, body);
    assert_eq!(r.kind, LayoutKind::Flat);
    assert_eq!(r.align, 1);
    // Static offsets: each earlier variable field counts its 8-byte prefix.
    assert_eq!(offsets(r), [0, 4, 12, 20]);
    let bytes = message_bytes();
    assert_eq!(extent(&table, body, &bytes), Ok(bytes.len() as u64));
    // Truncated bytes are an extent error, never a wild read.
    assert!(matches!(
        extent(&table, body, &bytes[..10]),
        Err(LayoutError::Extent(_))
    ));
    // A variable flat value lives behind a counted block of its bytes.
    let v = rec(&table, value);
    assert_eq!(v.kind, LayoutKind::Block { payload: body });
    assert_eq!(v.copy, CopyRecipe::FlatBytes);
    // A length prefix that would overflow the extent is checked.
    let mut huge = bytes.clone();
    huge[4..12].copy_from_slice(&u64::MAX.to_le_bytes());
    assert_eq!(extent(&table, body, &huge), Err(LayoutError::Overflow));
}

#[test]
fn checked_size_arithmetic_rejects_overflow() {
    let program = SizeSpec::Dynamic(vec![
        SizeTerm::Constant(8),
        SizeTerm::ReadLength {
            offset: 0,
            scalar: ScalarKind::U64,
        },
        SizeTerm::Constant(16),
        SizeTerm::Multiply,
        SizeTerm::Add,
    ]);
    let ok = 4u64.to_le_bytes();
    assert_eq!(evaluate_size(&program, &ok), Ok(72));
    let big = (u64::MAX / 8).to_le_bytes();
    assert_eq!(evaluate_size(&program, &big), Err(LayoutError::Overflow));
    assert_eq!(align_up(u64::MAX - 2, 8), Err(LayoutError::Overflow));
    assert!(matches!(align_up(8, 3), Err(LayoutError::Invalid(_))));
    assert!(matches!(
        evaluate_size(&SizeSpec::Dynamic(vec![SizeTerm::Add]), &[]),
        Err(LayoutError::Extent(_))
    ));
    assert_eq!(payload_offset(16), Ok(HEADER_SIZE));
    assert_eq!(payload_offset(64), Ok(8));
}

// ----- blocks, owners, closures, handles -----------------------------------

#[test]
fn recursion_terminates_only_through_an_explicit_block() {
    let mut owners = Owners::default();
    let tree = owner(&mut owners, "Tree", OwnerSpec::default(), |t| {
        let v = t.p(Primitive::I64);
        let me = t.named(&path("Tree"), vec![]);
        let children = t.list(me);
        t.record(&[v, children])
    });
    let cons = owner(&mut owners, "Cons", OwnerSpec::default(), |t| {
        let v = t.p(Primitive::I64);
        let me = t.named(&path("Cons"), vec![]);
        let tail = t.boxed(me);
        t.variant(&[&[], &[v, tail]])
    });
    let bad = owner(&mut owners, "Bad", OwnerSpec::default(), |t| {
        let me = t.named(&path("Bad"), vec![]);
        let v = t.p(Primitive::I64);
        t.record(&[v, me])
    });
    let alias = owner(&mut owners, "Alias", OwnerSpec::default(), |t| {
        t.named(&path("Alias"), vec![])
    });
    let mut b = LayoutBuilder::new(&x86(), &owners);
    let mut t = ty();
    let (tr, co, ba, al) = (
        t.named(&tree, vec![]),
        t.named(&cons, vec![]),
        t.named(&bad, vec![]),
        t.named(&alias, vec![]),
    );
    let tid = b.layout(&t.store, tr).unwrap();
    let cid = b.layout(&t.store, co).unwrap();
    assert_eq!(
        b.layout(&t.store, ba),
        Err(LayoutError::InfiniteSize("m.Bad".into()))
    );
    assert!(matches!(
        b.layout(&t.store, al),
        Err(LayoutError::InfiniteSize(_))
    ));
    let table = checked(b);
    let r = rec(&table, tid);
    let LayoutKind::Block { payload } = rec(&table, r.fields[1].layout).kind else {
        panic!("children is a block")
    };
    assert_eq!(
        rec(&table, payload).kind,
        LayoutKind::Sequence { element: tid }
    );
    // The element's drop (its own children) runs per element.
    assert!(
        matches!(rec(&table, payload).drop[..], [DropStep::ElementsReverse { element, stride: 16, .. }] if element == tid)
    );
    // `Nil | Cons(i64, Box[Cons])`: the box's null word is the tag.
    assert_eq!(fixed(rec(&table, cid)), 16);
    assert_eq!(
        rec(&table, cid).tag.as_ref().map(|t| (t.offset, t.form)),
        Some((
            8,
            TagForm::Niche {
                nullary: 0,
                dataful: 1
            }
        ))
    );
}

#[test]
fn opaque_containers_take_their_payload_from_the_owner() {
    let mut owners = Owners::default();
    owners.containers.insert(
        Container::Map,
        representation(
            "Map",
            OwnerSpec {
                generics: 2,
                ..OwnerSpec::default()
            },
            |t| {
                let (k, v, u64_, u32_) = (
                    t.param(0),
                    t.param(1),
                    t.p(Primitive::U64),
                    t.p(Primitive::U32),
                );
                let key = t.inline(u64_, 2);
                let entry = t.tuple(&[k, v]);
                let entries = t.list(entry);
                let index = t.list(u32_);
                t.record(&[key, entries, index])
            },
        ),
    );
    let mut b = LayoutBuilder::new(&x86(), &owners);
    let mut t = ty();
    let (s, i) = (t.p(Primitive::String), t.p(Primitive::I64));
    let map = t.builtin(
        BuiltinType::Map,
        vec![GenericArgument::Type(s), GenericArgument::Type(i)],
    );
    let set = t.builtin(BuiltinType::Set, vec![GenericArgument::Type(s)]);
    let id = b.layout(&t.store, map).unwrap();
    assert_eq!(
        b.layout(&t.store, set),
        Err(LayoutError::MissingOwner("Set".into()))
    );
    let table = checked(b);
    let r = rec(&table, id);
    assert_eq!(r.copy, CopyRecipe::Dup);
    let LayoutKind::Block { payload } = r.kind else {
        panic!("Map is a block")
    };
    let p = rec(&table, payload);
    assert_eq!(p.kind, LayoutKind::Opaque { owner: path("Map") });
    assert_eq!(offsets(p), [0, 16, 24]);
    assert_eq!(
        p.scan,
        [
            ScanStep::Counted { offset: 16 },
            ScanStep::Counted { offset: 24 }
        ]
    );
    assert!(p.key.owner_revision.is_some());
}

#[test]
fn closure_and_iterator_environments_carry_captures_and_sizes() {
    let owners = Owners::default();
    let mut b = LayoutBuilder::new(&x86(), &owners);
    let mut t = ty();
    let (i64_, string, f64_) = (
        t.p(Primitive::I64),
        t.p(Primitive::String),
        t.p(Primitive::F64),
    );
    let boxed = t.boxed(f64_);
    let body = path("main");
    let captures = [
        (i64_, hirsel_core_contract::Passing::Take),
        (string, hirsel_core_contract::Passing::Borrow),
        (boxed, hirsel_core_contract::Passing::Take),
        (string, hirsel_core_contract::Passing::Mut),
    ];
    let shape = |sizes: &'static [u64]| EnvironmentShape {
        definition: DefId::intern(&body),
        body: 3,
        captures: &captures,
        sizes,
        movable: &[],
    };
    let env = b.environment(&t.store, &shape(&[4])).unwrap();
    let env5 = b.environment(&t.store, &shape(&[5])).unwrap();
    assert_eq!(b.environment(&t.store, &shape(&[4])).unwrap(), env);
    let many = b.closure(env, hirsel_lir::abi_types::CallMultiplicity::Many);
    let once = b.closure(env, hirsel_lir::abi_types::CallMultiplicity::Once);
    let f = t.function(&[(PassingMode::Borrow, i64_)], i64_, Multiplicity::Many, 0);
    let erased = b.layout(&t.store, f).unwrap();
    let iter = t.builtin(BuiltinType::Iter, vec![GenericArgument::Type(string)]);
    let it = b.layout(&t.store, iter).unwrap();
    let table = checked(b);
    assert_ne!(env, env5, "captured sizes are part of the environment key");
    let r = rec(&table, env);
    // A borrowed capture is the captured view itself (the 24-byte String
    // slice, its owner scanned as uncounted); a mut one is an address.
    assert_eq!(offsets(r), [0, 8, 32, 40]);
    assert_eq!(fixed(r), 48);
    assert_eq!(
        r.scan,
        [
            ScanStep::Uncounted { offset: 8 },
            ScanStep::Counted { offset: 32 },
            ScanStep::Uncounted { offset: 40 }
        ]
    );
    // Only the owned captures are released, high to low.
    assert_eq!(r.drop, [DropStep::Release { offset: 32 }]);
    let m = rec(&table, many);
    assert_eq!(
        m.kind,
        LayoutKind::Callable {
            environment: Some(env)
        }
    );
    assert_eq!((fixed(m), m.copy), (16, CopyRecipe::Dup));
    assert_eq!(rec(&table, once).copy, CopyRecipe::NotCopyable);
    assert_eq!(
        rec(&table, erased).kind,
        LayoutKind::Callable { environment: None }
    );
    let i = rec(&table, it);
    assert_eq!((fixed(i), i.copy), (24, CopyRecipe::NotCopyable));
    assert_eq!(i.drop, [DropStep::Environment { offset: 0 }]);
}

#[test]
fn handles_are_ids_and_resource_handles_close() {
    let mut owners = Owners::default();
    owners.nominals.insert(
        DefId::intern(&path("MonotonicClock")),
        OwnerRepresentation::Handle {
            revision: ContentHash::of_bytes(b"clock"),
            resource: false,
        },
    );
    owners.nominals.insert(
        DefId::intern(&path("Socket")),
        OwnerRepresentation::Handle {
            revision: ContentHash::of_bytes(b"socket"),
            resource: true,
        },
    );
    let mut b = LayoutBuilder::new(&x86(), &owners);
    let mut t = ty();
    let (c, s) = (
        t.named(&path("MonotonicClock"), vec![]),
        t.named(&path("Socket"), vec![]),
    );
    let cid = b.layout(&t.store, c).unwrap();
    let sid = b.layout(&t.store, s).unwrap();
    let table = checked(b);
    assert_eq!(rec(&table, cid).copy, CopyRecipe::Bits);
    assert!(rec(&table, cid).drop.is_empty());
    assert_eq!(rec(&table, sid).drop, [DropStep::CloseHandle { offset: 0 }]);
}

// ----- identity -----------------------------------------------------------

#[test]
fn relations_never_change_a_layout() {
    let mut owners = Owners::default();
    let pair = owner(
        &mut owners,
        "Pair",
        OwnerSpec {
            generics: 1,
            ..OwnerSpec::default()
        },
        |t| {
            let a = t.param(0);
            t.record(&[a, a])
        },
    );
    let mut b = LayoutBuilder::new(&x86(), &owners);
    let mut t = ty();
    let rel = |i| Modes {
        relation: Relation::parameters([BinderRef::bound(ParamId(i))]),
        uniqueness: if i == 0 {
            Uniqueness::Unique
        } else {
            Uniqueness::Aliased
        },
        ..Modes::global()
    };
    let s0 = t.add_with(TypeKind::Primitive(Primitive::String), rel(0));
    let mut qualified = t.store.get(s0).unwrap().clone();
    qualified.sendability_requirement = SendabilityRequirement::Sendable;
    let s0 = t.store.intern(qualified);
    let s1 = t.add_with(TypeKind::Primitive(Primitive::String), rel(1));
    let p0 = t.named(&pair, vec![GenericArgument::Type(s0)]);
    let p1 = t.named(&pair, vec![GenericArgument::Type(s1)]);
    let a = b.layout(&t.store, p0).unwrap();
    assert_eq!(b.layout(&t.store, p1).unwrap(), a);
    let bytes = t.p(Primitive::Bytes);
    let pb = t.named(&pair, vec![GenericArgument::Type(bytes)]);
    assert_eq!(b.layout(&t.store, pb).unwrap(), a);
    let byte = t.p(Primitive::U8);
    let different = t.named(&pair, vec![GenericArgument::Type(byte)]);
    assert_ne!(b.layout(&t.store, different).unwrap(), a);
    checked(b);
}

#[test]
fn incomplete_and_foreign_types_are_named() {
    let mut owners = Owners::default();
    let unavailable_cases: [(&str, Layout, bool, Option<u32>); 3] = [
        (
            "PackAlign",
            Layout {
                packed: true,
                align: Some(8),
                ..Layout::default()
            },
            false,
            Some(47),
        ),
        (
            "FlatC",
            Layout {
                flat: true,
                c: true,
                ..Layout::default()
            },
            false,
            Some(47),
        ),
        (
            "SoaC",
            Layout {
                soa: true,
                c: true,
                ..Layout::default()
            },
            false,
            Some(47),
        ),
    ];
    for (name, promises, variant, _) in &unavailable_cases {
        owner(
            &mut owners,
            name,
            OwnerSpec {
                promises: promises.clone(),
                ..OwnerSpec::default()
            },
            |t| {
                let a = t.p(Primitive::U32);
                if *variant {
                    t.variant(&[&[a]])
                } else {
                    t.record(&[a])
                }
            },
        );
    }
    let soa = owner(
        &mut owners,
        "SoaRec",
        OwnerSpec {
            promises: Layout {
                soa: true,
                ..Layout::default()
            },
            ..OwnerSpec::default()
        },
        |t| {
            let a = t.p(Primitive::U32);
            t.record(&[a])
        },
    );
    // MEM-7: an @soa variant and a C variant (a C tag, then a union of the arms'
    // structs) both derive.
    let mut supported = Vec::new();
    for (name, promises) in [
        (
            "SoaEnum",
            Layout {
                soa: true,
                ..Layout::default()
            },
        ),
        (
            "CEnum",
            Layout {
                c: true,
                ..Layout::default()
            },
        ),
    ] {
        supported.push(owner(
            &mut owners,
            name,
            OwnerSpec {
                promises,
                ..OwnerSpec::default()
            },
            |t| {
                let (a, b) = (t.p(Primitive::U32), t.p(Primitive::U64));
                t.variant(&[&[a], &[b], &[]])
            },
        ));
    }
    let cbox = owner(
        &mut owners,
        "CBox",
        OwnerSpec {
            promises: Layout {
                c: true,
                ..Layout::default()
            },
            ..OwnerSpec::default()
        },
        |t| {
            let a = t.p(Primitive::String);
            t.record(&[a])
        },
    );
    let mut b = LayoutBuilder::new(&x86(), &owners);
    let mut t = ty();
    for (name, _, _, issue) in &unavailable_cases {
        let ty = t.named(&path(name), vec![]);
        match b.layout(&t.store, ty) {
            Err(LayoutError::Unavailable { issue: i, .. }) => assert_eq!(i, *issue, "{name}"),
            other => panic!("{name}: {other:?}"),
        }
    }
    for enumeration in &supported {
        let mut builder = LayoutBuilder::new(&x86(), &owners);
        let e = t.named(enumeration, vec![]);
        let id = builder.layout(&t.store, e).unwrap();
        let record = builder.record(id);
        let tag = record.tag.as_ref().unwrap();
        assert_eq!((tag.offset, tag.form), (0, TagForm::Ordinal));
        assert_eq!(
            (record.size.clone(), record.align),
            (SizeSpec::Fixed(16), 8)
        );
    }
    let s = t.named(&soa, vec![]);
    let inline_soa = t.inline(s, 4);
    assert!(matches!(
        b.layout(&t.store, inline_soa),
        Err(LayoutError::Unavailable {
            issue: Some(47),
            ..
        })
    ));
    let c = t.named(&cbox, vec![]);
    assert!(matches!(
        b.layout(&t.store, c),
        Err(LayoutError::Unavailable { issue: Some(6), .. })
    ));
    // Mutex wrappers use atomic roots for scalar, counted, and resource payloads.
    let u8_ = t.p(Primitive::U8);
    let sender = t.builtin(BuiltinType::Sender, vec![GenericArgument::Type(u8_)]);
    let mutex = t.builtin(BuiltinType::Mutex, vec![GenericArgument::Type(sender)]);
    let plain = t.builtin(BuiltinType::Mutex, vec![GenericArgument::Type(u8_)]);
    let text = t.p(Primitive::String);
    let owned = t.builtin(BuiltinType::Mutex, vec![GenericArgument::Type(text)]);
    let free = t.param(0);
    assert_eq!(b.layout(&t.store, free), Err(LayoutError::Incomplete));
    let missing = t.named(&path("Nowhere"), vec![]);
    assert_eq!(
        b.layout(&t.store, missing),
        Err(LayoutError::MissingOwner("m.Nowhere".into()))
    );
    // Failed requests leave no partial records behind.
    let table = checked(b);
    assert!(table.layouts.len() <= 2, "{} records", table.layouts.len());
    for accepted in [plain, owned, mutex] {
        let mut builder = LayoutBuilder::new(&x86(), &owners);
        let root = builder.layout(&t.store, accepted).unwrap();
        let table = checked(builder);
        let root = &table.layouts[root.0 as usize];
        assert_eq!(root.scan, [ScanStep::Root { offset: 0 }]);
        assert_eq!(root.drop, [DropStep::ReleaseRoot { offset: 0 }]);
        let LayoutKind::Shared { payload } = root.kind else {
            panic!("a mutex without an atomic root");
        };
        let LayoutKind::Frozen { value } = table.layouts[payload.0 as usize].kind else {
            panic!("a mutex without a frozen cell word");
        };
        assert_eq!(
            table.layouts[value.0 as usize].drop,
            [DropStep::CloseHandle { offset: 0 }]
        );
        assert_eq!(
            table.layouts[payload.0 as usize].drop,
            [DropStep::Child {
                offset: 0,
                layout: value
            }]
        );
    }
    // Raw and callback pointers are pointer scalars (PROG-4).
    let mut b = LayoutBuilder::new(&x86(), &owners);
    for kind in [BuiltinType::Ptr, BuiltinType::FnPtr] {
        let ty = t.builtin(kind, vec![GenericArgument::Type(u8_)]);
        let id = b.layout(&t.store, ty).unwrap();
        let table = b.table();
        assert_eq!(
            table.layouts[id.0 as usize].kind,
            LayoutKind::Scalar(ScalarKind::Pointer),
            "{kind:?}"
        );
    }
}

#[test]
fn private_representation_changes_move_layout_keys() {
    let build = |width: Primitive| {
        let mut owners = Owners::default();
        let spec = OwnerSpec {
            revision: if width == Primitive::U32 {
                "narrow"
            } else {
                "wide"
            },
            ..OwnerSpec::default()
        };
        owner(&mut owners, "Id", spec, |t| {
            let a = t.p(width);
            t.record(&[a])
        });
        owners
    };
    let (narrow, wide) = (build(Primitive::U32), build(Primitive::U64));
    let mut t = ty();
    let id = t.named(&path("Id"), vec![]);
    let holder = t.record(&[id, id]);
    let key = |owners: &Owners| {
        let mut b = LayoutBuilder::new(&x86(), owners);
        let h = b.layout(&t.store, holder).unwrap();
        let table = checked(b);
        rec(&table, h).key.clone()
    };
    assert_ne!(key(&narrow).instance, key(&wide).instance);
    assert_eq!(key(&narrow), key(&narrow));
}

// ----- validator mutations -------------------------------------------------

fn mutation_corpus() -> (LayoutTable, LayoutId, LayoutId, LayoutId) {
    let mut owners = Owners::default();
    let res = owner(
        &mut owners,
        "Res",
        OwnerSpec {
            resource: true,
            finalizer: true,
            ..OwnerSpec::default()
        },
        |t| {
            let (a, s) = (t.p(Primitive::U8), t.p(Primitive::String));
            t.record(&[a, s])
        },
    );
    let mut b = LayoutBuilder::new(&x86(), &owners);
    let mut t = ty();
    let (u16_, string, i64_) = (
        t.p(Primitive::U16),
        t.p(Primitive::String),
        t.p(Primitive::I64),
    );
    let bx = t.boxed(i64_);
    let rec_ty = t.record(&[u16_, string, bx]);
    let v = t.variant(&[&[u16_], &[i64_, string]]);
    let r = t.named(&res, vec![]);
    let a = b.layout(&t.store, rec_ty).unwrap();
    let vv = b.layout(&t.store, v).unwrap();
    let rr = b.layout(&t.store, r).unwrap();
    (checked(b), a, vv, rr)
}

fn rejects(table: &LayoutTable, obligation: Obligation) {
    let violations = validate_table(table, &x86()).expect_err("mutation must be rejected");
    assert!(
        violations.iter().any(|v| v.obligation == obligation),
        "expected {obligation:?}, got {violations:#?}"
    );
}

#[test]
fn the_validator_rejects_one_mutation_per_obligation() {
    let (table, aggregate, variant, resource) = mutation_corpus();
    let a = aggregate.0 as usize;
    let v = variant.0 as usize;
    let r = resource.0 as usize;

    let mut m = table.clone();
    m.layouts[a].fields[1].offset = 4; // misaligned String
    rejects(&m, Obligation::Offset);

    let mut m = table.clone();
    m.layouts[a].fields[2].offset = 16; // overlaps the String
    rejects(&m, Obligation::Overlap);

    let mut m = table.clone();
    m.layouts[v].tag.as_mut().unwrap().offset = 2;
    rejects(&m, Obligation::Tag);

    let mut m = table.clone();
    m.layouts[v].tag.as_mut().unwrap().variants[1].fields[0].offset = 0; // over the tag
    rejects(&m, Obligation::Tag);

    let mut m = table.clone();
    m.layouts[a].scan.pop(); // the box is no longer scanned
    rejects(&m, Obligation::Scan);

    let mut m = table.clone();
    let ScanStep::SliceOwner { offset } = &mut m.layouts[a].scan[0] else {
        panic!()
    };
    *offset += 8; // a scan edge at the wrong word
    rejects(&m, Obligation::Scan);

    let mut m = table.clone();
    m.layouts[a].drop.reverse(); // fields low to high
    rejects(&m, Obligation::DropOrder);

    let mut m = table.clone();
    m.layouts[r].drop.rotate_left(1); // finalizer after the fields
    rejects(&m, Obligation::DropOrder);

    let mut m = table.clone();
    m.layouts[a].size = SizeSpec::Fixed(48);
    rejects(&m, Obligation::Size);

    let mut m = table.clone();
    m.layouts[v].size = SizeSpec::Fixed(24);
    rejects(&m, Obligation::Size);

    let mut m = table.clone();
    m.layouts[a].align = 4;
    rejects(&m, Obligation::Alignment);

    let mut m = table.clone();
    m.layouts[a].align = 3;
    rejects(&m, Obligation::Alignment);

    let mut m = table.clone();
    m.layouts[r].copy = CopyRecipe::Fields; // a copyable resource
    rejects(&m, Obligation::Copy);

    let mut m = table.clone();
    let child = m.layouts[a].fields[0].layout;
    m.layouts[child.0 as usize].fields.push(FieldLayout {
        ordinal: 0,
        offset: 0,
        layout: aggregate,
        unaligned: false,
    });
    rejects(&m, Obligation::Recursion);

    let mut m = table.clone();
    m.layouts[a].fields[0].layout = LayoutId(10_000);
    rejects(&m, Obligation::Reference);

    let mut m = table.clone();
    m.layouts[a].size = SizeSpec::Dynamic(vec![SizeTerm::Add]);
    rejects(&m, Obligation::SizeProgram);
}

// ----- golden vectors and C probes -----------------------------------------

/// A corpus covering every family, derived for one target.
fn golden_corpus(target: &TargetSpec) -> LayoutTable {
    let mut owners = Owners::default();
    let message = flat_message(&mut owners);
    let tree = owner(&mut owners, "Tree", OwnerSpec::default(), |t| {
        let me = t.named(&path("Tree"), vec![]);
        let child = t.boxed(me);
        t.variant(&[&[], &[child, child]])
    });
    let c = c_records(&mut owners);
    let mut b = LayoutBuilder::new(target, &owners);
    let mut t = ty();
    let (u8_, i128_, string, f64_) = (
        t.p(Primitive::U8),
        t.p(Primitive::I128),
        t.p(Primitive::String),
        t.p(Primitive::F64),
    );
    let bx = t.boxed(f64_);
    let v = t.variant(&[&[u8_], &[i128_, string]]);
    let packed = t.builtin(BuiltinType::Packed, vec![GenericArgument::Type(v)]);
    let inline = t.inline(string, 2);
    let list = t.list(bx);
    let m = t.named(&message, vec![]);
    let f = t.function(&[(PassingMode::Take, string)], u8_, Multiplicity::Once, 0);
    let iter = t.builtin(BuiltinType::Iter, vec![GenericArgument::Type(u8_)]);
    let all = t.record(&[u8_, i128_, string, bx, v, inline, list, m, f, iter]);
    b.layout(&t.store, all).unwrap();
    b.layout(&t.store, packed).unwrap();
    b.flat(&t.store, m).unwrap();
    let node = t.named(&tree, vec![]);
    let node = t.boxed(node);
    b.layout(&t.store, node).unwrap();
    // A `Shared` root over a frozen list of boxes (MEM-11).
    let shared = t.builtin(BuiltinType::Shared, vec![GenericArgument::Type(list)]);
    b.layout(&t.store, shared).unwrap();
    let mutex = t.builtin(BuiltinType::Mutex, vec![GenericArgument::Type(string)]);
    b.layout(&t.store, mutex).unwrap();
    b.header();
    for name in c {
        let ty = t.named(&path(name), vec![]);
        b.layout(&t.store, ty).unwrap();
    }
    checked(b)
}

fn golden_dir() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../native-tests/layout")
}

/// Compare with the committed golden text; `HIRSEL_BLESS=1` rewrites it.
fn golden(name: &str, actual: &str) {
    let file = golden_dir().join(name);
    if std::env::var_os("HIRSEL_BLESS").is_some() {
        std::fs::write(&file, actual).unwrap();
        return;
    }
    let expected =
        std::fs::read_to_string(&file).unwrap_or_else(|e| panic!("{}: {e}", file.display()));
    assert!(
        expected == actual,
        "{name} differs from the derivation; rerun with HIRSEL_BLESS=1 after review"
    );
}

/// `@layout(c)` records mirrored by hand in `native-tests/layout/c_records.h`.
fn c_records(owners: &mut Owners) -> Vec<&'static str> {
    let c = || OwnerSpec {
        promises: Layout {
            c: true,
            ..Layout::default()
        },
        ..OwnerSpec::default()
    };
    let cp = || OwnerSpec {
        promises: Layout {
            c: true,
            packed: true,
            ..Layout::default()
        },
        ..OwnerSpec::default()
    };
    let ca = || OwnerSpec {
        promises: Layout {
            c: true,
            align: Some(32),
            ..Layout::default()
        },
        ..OwnerSpec::default()
    };
    owner(owners, "CHeader", c(), |t| {
        let (a, b) = (t.p(Primitive::U32), t.p(Primitive::U16));
        t.record(&[a, b, b])
    });
    owner(owners, "CMixed", c(), |t| {
        let (a, b, d, e, f) = (
            t.p(Primitive::U8),
            t.p(Primitive::F64),
            t.p(Primitive::Bool),
            t.p(Primitive::I16),
            t.p(Primitive::F32),
        );
        t.record(&[a, b, d, e, f])
    });
    owner(owners, "CWide", c(), |t| {
        let (a, b, d) = (
            t.p(Primitive::U8),
            t.p(Primitive::I128),
            t.p(Primitive::U32),
        );
        t.record(&[a, b, d])
    });
    owner(owners, "CNested", c(), |t| {
        let (a, h) = (t.p(Primitive::U8), t.named(&path("CMixed"), vec![]));
        let arr = t.inline(h, 3);
        let bytes = t.inline(a, 5);
        t.record(&[a, h, bytes, arr])
    });
    owner(owners, "CPacked", cp(), |t| {
        let (a, b, d) = (t.p(Primitive::U8), t.p(Primitive::U64), t.p(Primitive::U16));
        t.record(&[a, b, d])
    });
    owner(owners, "CAligned", ca(), |t| {
        let (a, b) = (t.p(Primitive::U8), t.p(Primitive::U64));
        t.record(&[a, b])
    });
    vec![
        "CHeader", "CMixed", "CWide", "CNested", "CPacked", "CAligned",
    ]
}

/// Independent C probes: clang computes `sizeof`/`alignof`/`offsetof` of the
/// hand-written C mirrors and must agree with the derivation on both targets.
#[test]
#[ignore = "native: run by tools/check-native.sh"]
fn c_probes_agree_on_both_targets() {
    for (target, triple) in TargetSpec::executed()
        .into_iter()
        .zip(["x86_64-linux-gnu", "aarch64-linux-gnu"])
    {
        let mut owners = Owners::default();
        let names = c_records(&mut owners);
        let mut b = LayoutBuilder::new(&target, &owners);
        let mut t = ty();
        let mut probe = String::from("#include \"c_records.h\"\n");
        let mut ids = Vec::new();
        for name in &names {
            let ty = t.named(&path(name), vec![]);
            ids.push(b.layout(&t.store, ty).unwrap());
        }
        let table = checked(b);
        // The niche shapes: the dataful variant's fields are the whole value,
        // and the niche word is a pointer word inside them.
        let (niches, tree, _, shifted) = niche_table_for(&target);
        for (name, id) in [("NicheTree", tree), ("NicheShifted", shifted)] {
            let r = rec(&niches, id);
            let tag = r.tag.as_ref().unwrap();
            let TagForm::Niche { dataful, .. } = tag.form else {
                panic!("{name} is a niche")
            };
            probe += &format!(
                "_Static_assert(sizeof(struct {name}) == {}, \"{name} size\");\n",
                fixed(r)
            );
            probe += &format!(
                "_Static_assert(_Alignof(struct {name}) == {}, \"{name} align\");\n",
                r.align
            );
            for f in &tag.variants[dataful as usize].fields {
                probe += &format!(
                    "_Static_assert(offsetof(struct {name}, f{0}) == {1}, \"{name}.f{0}\");\n",
                    f.ordinal, f.offset
                );
            }
            probe += &format!(
                "_Static_assert(offsetof(struct {name}, niche) == {}, \"{name} niche\");\n",
                tag.offset
            );
        }
        for (name, id) in names.iter().zip(ids) {
            let r = rec(&table, id);
            probe += &format!(
                "_Static_assert(sizeof(struct {name}) == {}, \"{name} size\");\n",
                fixed(r)
            );
            probe += &format!(
                "_Static_assert(_Alignof(struct {name}) == {}, \"{name} align\");\n",
                r.align
            );
            assert_eq!(r.initialized, InitializedBytes::ZeroPadding);
            for (i, f) in r.fields.iter().enumerate() {
                probe += &format!(
                    "_Static_assert(offsetof(struct {name}, f{i}) == {}, \"{name}.f{i}\");\n",
                    f.offset
                );
            }
        }
        let dir = std::env::temp_dir().join(format!("hirsel-layout-probe-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join(format!("probe-{triple}.c"));
        std::fs::write(&file, &probe).unwrap();
        let out = std::process::Command::new(
            std::path::PathBuf::from(
                std::env::var_os("LLVM_SYS_231_PREFIX").expect("LLVM_SYS_231_PREFIX"),
            )
            .join("bin/clang"),
        )
        .args(["-std=c11", "-ffreestanding", "-fsyntax-only"])
        .arg(format!("--target={triple}"))
        .arg("-I")
        .arg(golden_dir())
        .arg(&file)
        .output()
        .expect("the pinned clang input is executable");
        assert!(
            out.status.success(),
            "{triple}: {}\n{probe}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
}

// ----- request order ------------------------------------------------------

/// Each record with every `LayoutId` it names rewritten to the rank of the
/// named record's key, sorted by key: equal for two tables exactly when they
/// hold the same records whatever order derived them.
fn canonical(table: &LayoutTable) -> Vec<LayoutRecord> {
    let key = |r: &LayoutRecord| snapshot::to_text(&r.key);
    let mut order: Vec<usize> = (0..table.layouts.len()).collect();
    order.sort_by_cached_key(|&i| key(&table.layouts[i]));
    let mut rank = vec![0u32; order.len()];
    for (r, &i) in order.iter().enumerate() {
        rank[i] = r as u32;
    }
    let map = |id: &mut LayoutId| *id = LayoutId(rank[id.0 as usize]);
    order
        .iter()
        .map(|&i| {
            let mut r = table.layouts[i].clone();
            r.for_each_reference_mut(&mut |id: &mut LayoutId| map(id));
            r
        })
        .collect()
}

/// Every permutation of `0..n` (Heap's algorithm).
fn permutations(n: usize) -> Vec<Vec<usize>> {
    fn go(k: usize, a: &mut Vec<usize>, out: &mut Vec<Vec<usize>>) {
        if k <= 1 {
            out.push(a.clone());
            return;
        }
        for i in 0..k - 1 {
            go(k - 1, a, out);
            let j = if k % 2 == 0 { i } else { 0 };
            a.swap(j, k - 1);
        }
        go(k - 1, a, out);
    }
    let mut out = Vec::new();
    go(n, &mut (0..n).collect(), &mut out);
    out
}

/// Deriving a set of recursive types yields the same records, and the same
/// answer to each request, in every request order. `Box[Cons]` first used to
/// reserve the block while `Cons` was pending and report an infinite size.
#[test]
fn recursive_layouts_do_not_depend_on_request_order() {
    let mut owners = Owners::default();
    let tree = owner(&mut owners, "Tree", OwnerSpec::default(), |t| {
        let v = t.p(Primitive::I64);
        let me = t.named(&path("Tree"), vec![]);
        let children = t.list(me);
        t.record(&[v, children])
    });
    let cons = owner(&mut owners, "Cons", OwnerSpec::default(), |t| {
        let v = t.p(Primitive::I64);
        let me = t.named(&path("Cons"), vec![]);
        let tail = t.boxed(me);
        t.variant(&[&[], &[v, tail]])
    });
    // Mutual recursion through a box on one side only.
    let even = owner(&mut owners, "Even", OwnerSpec::default(), |t| {
        let odd = t.named(&path("Odd"), vec![]);
        let next = t.boxed(odd);
        t.variant(&[&[], &[next]])
    });
    let odd = owner(&mut owners, "Odd", OwnerSpec::default(), |t| {
        let even = t.named(&path("Even"), vec![]);
        let v = t.p(Primitive::U8);
        t.record(&[v, even])
    });
    // A deferred Box payload that is also held inline.
    let pair = owner(&mut owners, "Pair", OwnerSpec::default(), |t| {
        let c = t.named(&path("Cons"), vec![]);
        let b = t.boxed(c);
        t.record(&[b, c])
    });
    let bad = owner(&mut owners, "Bad", OwnerSpec::default(), |t| {
        let me = t.named(&path("Bad"), vec![]);
        t.record(&[me])
    });
    let mut t = ty();
    let c = t.named(&cons, vec![]);
    let boxed_cons = t.boxed(c);
    let o = t.named(&odd, vec![]);
    let tr = t.named(&tree, vec![]);
    let requests = [
        boxed_cons,
        t.boxed(boxed_cons),
        t.named(&pair, vec![]),
        tr,
        t.boxed(o),
        t.named(&even, vec![]),
        t.named(&bad, vec![]),
    ];
    let derive = |order: &[usize]| {
        let mut b = LayoutBuilder::new(&x86(), &owners);
        let mut answers = vec![None; requests.len()];
        for &i in order {
            answers[i] = Some(b.layout(&t.store, requests[i]).map(|_| ()));
        }
        (answers, canonical(&checked(b)))
    };
    let first = derive(&(0..requests.len()).collect::<Vec<_>>());
    assert!(
        first.0[..6].iter().all(|a| a == &Some(Ok(()))),
        "{:?}",
        first.0
    );
    assert_eq!(
        first.0[6],
        Some(Err(LayoutError::InfiniteSize("m.Bad".into())))
    );
    for order in permutations(requests.len()) {
        assert!(derive(&order) == first, "request order {order:?}");
    }
}

/// Content keys (design §1e): `Cons` and mutually recursive `Even`/`Odd` get the same
/// `LayoutKey` whatever order they are requested in and whatever unrelated layouts the
/// table holds first, although their `LayoutId`s differ; editing one member changes the
/// key of exactly the layouts that reach it.
#[test]
fn layout_keys_depend_on_content_alone() {
    let owners = |odd_field: Primitive| {
        let mut owners = Owners::default();
        owner(&mut owners, "Cons", OwnerSpec::default(), |t| {
            let v = t.p(Primitive::I64);
            let me = t.named(&path("Cons"), vec![]);
            let tail = t.boxed(me);
            t.variant(&[&[], &[v, tail]])
        });
        owner(&mut owners, "Even", OwnerSpec::default(), |t| {
            let odd = t.named(&path("Odd"), vec![]);
            let next = t.boxed(odd);
            t.variant(&[&[], &[next]])
        });
        owner(&mut owners, "Odd", OwnerSpec::default(), |t| {
            let even = t.named(&path("Even"), vec![]);
            let v = t.p(odd_field);
            t.record(&[v, even])
        });
        owners
    };
    let mut t = ty();
    let cons = t.named(&path("Cons"), vec![]);
    let even = t.named(&path("Even"), vec![]);
    let odd = t.named(&path("Odd"), vec![]);
    let requests = [cons, t.boxed(cons), even, odd, t.boxed(even)];
    let i32_ = t.p(Primitive::I32);
    let u16_ = t.p(Primitive::U16);
    let list = t.list(i32_);
    let unrelated = [list, t.record(&[u16_, list]), t.boxed(u16_)];
    let derive = |owners: &Owners, order: &[usize], first: &[TypeId]| {
        let mut b = LayoutBuilder::new(&x86(), owners);
        for ty in first {
            b.layout(&t.store, *ty).unwrap();
        }
        let mut ids = vec![LayoutId(0); requests.len()];
        for &i in order {
            ids[i] = b.layout(&t.store, requests[i]).unwrap();
        }
        let table = checked(b);
        let keys = hirsel_lir::LayoutKeys::new(&table);
        for index in 0..table.layouts.len() {
            let id = LayoutId(index as u32);
            assert_eq!(hirsel_lir::LayoutKeys::rooted(&table, id), keys.key(id));
        }
        let named: Vec<LayoutKey> = ids.iter().map(|id| keys.key(*id)).collect();
        (named, ids, table, keys)
    };
    let plain = owners(Primitive::U8);
    let (base, base_ids, table, keys) = derive(&plain, &[0, 1, 2, 3, 4], &[]);
    let (reversed, reversed_ids, ..) = derive(&plain, &[4, 3, 2, 1, 0], &[]);
    let (inserted, inserted_ids, ..) = derive(&plain, &[4, 3, 2, 1, 0], &unrelated);
    assert_ne!(
        base_ids, reversed_ids,
        "the request order renumbers the table"
    );
    assert_ne!(
        reversed_ids, inserted_ids,
        "unrelated layouts renumber the table"
    );
    assert_eq!(base, reversed);
    assert_eq!(base, inserted);
    let distinct: std::collections::BTreeSet<_> = keys.all().iter().collect();
    assert_eq!(
        distinct.len(),
        keys.all().len(),
        "distinct layouts, distinct keys"
    );

    // Editing `Odd` changes exactly the keys of the layouts that reach it.
    let (edited, edited_ids, edited_table, edited_keys) =
        derive(&owners(Primitive::U16), &[0, 1, 2, 3, 4], &[]);
    assert_eq!(edited_ids, base_ids);
    assert_eq!(edited_table.layouts.len(), table.layouts.len());
    let odd_id = base_ids[3];
    let reaches = |from: LayoutId| {
        let mut seen = std::collections::BTreeSet::new();
        let mut pending = vec![from];
        while let Some(id) = pending.pop() {
            if seen.insert(id) {
                pending.extend(rec(&table, id).references());
            }
        }
        seen.contains(&odd_id)
    };
    for id in (0..table.layouts.len() as u32).map(LayoutId) {
        let what = format!("layout {id:?}: {:?}", rec(&table, id).kind);
        if reaches(id) {
            assert_ne!(keys.key(id), edited_keys.key(id), "{what}");
        } else if rec(&table, id) == rec(&edited_table, id) {
            assert_eq!(keys.key(id), edited_keys.key(id), "{what}");
        }
    }
    assert_eq!(edited[0..2], base[0..2], "Cons does not reach Odd");
    assert!(edited[2..].iter().zip(&base[2..]).all(|(a, b)| a != b));
}

/// A borrowed capture's field is the captured view, its holders scanned as
/// uncounted and never dropped by the environment (LIR schema revision 3);
/// the validator reads such a field as borrowed only when both its scan and
/// its drop program say so.
#[test]
fn a_borrowed_capture_field_is_scanned_uncounted_and_never_dropped() {
    let owners = Owners::default();
    let mut b = LayoutBuilder::new(&x86(), &owners);
    let mut t = ty();
    let i64_ = t.p(Primitive::I64);
    let boxed = t.boxed(i64_);
    let body = path("main");
    let captures = [
        (boxed, hirsel_core_contract::Passing::Borrow),
        (boxed, hirsel_core_contract::Passing::Take),
    ];
    let env = b
        .environment(
            &t.store,
            &EnvironmentShape {
                definition: DefId::intern(&body),
                body: 1,
                captures: &captures,
                sizes: &[],
                movable: &[],
            },
        )
        .unwrap();
    let table = checked(b);
    let e = env.0 as usize;
    let r = &table.layouts[e];
    assert_eq!(
        r.fields[0].layout, r.fields[1].layout,
        "the view's own layout"
    );
    assert_eq!(
        r.scan,
        [
            ScanStep::Uncounted { offset: 0 },
            ScanStep::Counted { offset: 8 }
        ]
    );
    assert_eq!(r.drop, [DropStep::Release { offset: 8 }]);

    let mut m = table.clone();
    m.layouts[e].drop.insert(1, DropStep::Release { offset: 0 }); // the borrow released
    rejects(&m, Obligation::Scan);

    let mut m = table.clone();
    m.layouts[e].scan[1] = ScanStep::Uncounted { offset: 8 }; // an owned box read as borrowed
    rejects(&m, Obligation::Scan);
}

/// An environment's runtime descriptor holds exactly its drop program: a
/// Take capture's steps, and none for a Borrow or a Mut capture, whose fields
/// the environment never drops. Each step names the field whose bytes it acts
/// in.
#[test]
fn an_environment_descriptor_drops_only_its_taken_captures() {
    use hirsel_core_contract::Passing;
    use hirsel_lir::rt_program::{StepKind, descriptor};
    let owners = Owners::default();
    let mut b = LayoutBuilder::new(&x86(), &owners);
    let mut t = ty();
    let i64_ = t.p(Primitive::I64);
    let boxed = t.boxed(i64_);
    let body = path("main");
    for captures in [
        [
            (boxed, Passing::Take),
            (boxed, Passing::Borrow),
            (boxed, Passing::Mut),
        ],
        [
            (boxed, Passing::Mut),
            (boxed, Passing::Borrow),
            (boxed, Passing::Take),
        ],
    ] {
        let env = b
            .environment(
                &t.store,
                &EnvironmentShape {
                    definition: DefId::intern(&body),
                    body: 1,
                    captures: &captures,
                    sizes: &[],
                    movable: &[],
                },
            )
            .unwrap();
        let table = b.table().clone();
        let r = &table.layouts[env.0 as usize];
        let taken = captures
            .iter()
            .position(|(_, p)| *p == Passing::Take)
            .unwrap();
        let offset = r.fields[taken].offset;
        assert_eq!(r.drop, [DropStep::Release { offset }]);
        let d = descriptor(&table, env).unwrap();
        let steps: Vec<_> = d
            .steps
            .iter()
            .map(|s| (s.kind, s.offset, s.field))
            .collect();
        assert_eq!(steps, [(StepKind::ReleaseLast, offset, taken as u32)]);
    }
}

/// A sealed table always holds `Scalar(Bool)`, even when nothing laid out
/// names a bool: a uniqueness test after sealing needs it. It is appended
/// last, so a table that already had one keeps its ids.
#[test]
fn every_sealed_table_holds_a_bool_layout() {
    let owners = Owners::default();
    let mut b = LayoutBuilder::new(&x86(), &owners);
    let mut t = ty();
    let u64_ = t.p(Primitive::U64);
    let boxed = t.boxed(u64_);
    let before = b.layout(&t.store, boxed).unwrap();
    let table = checked(b);
    let bools: Vec<_> = table
        .layouts
        .iter()
        .filter(|l| l.kind == LayoutKind::Scalar(ScalarKind::Bool))
        .collect();
    assert_eq!(bools.len(), 1);
    assert!(rec(&table, before).kind != LayoutKind::Scalar(ScalarKind::Bool));
    let empty = checked(LayoutBuilder::new(&x86(), &owners));
    assert_eq!(empty.layouts.len(), 1);
    assert_eq!(empty.layouts[0].kind, LayoutKind::Scalar(ScalarKind::Bool));
}

/// A borrowed capture of a resource holding a handle keeps the handle as a
/// `Handle` scan step, as the owned field would, so its descriptor tells it
/// from an owned field missing its drop steps by `borrowed_scan`, as the
/// validator's `borrowed_fields` does: the borrowed field contributes no
/// steps. An owned box whose release is removed still has a counted holder
/// in its bytes and is refused.
#[test]
fn a_borrowed_handle_holding_capture_contributes_no_descriptor_steps() {
    use hirsel_core_contract::Passing;
    use hirsel_lir::rt_program::{DescriptorError, StepKind, descriptor};
    let mut owners = Owners::default();
    owners.nominals.insert(
        DefId::intern(&path("Stdout")),
        OwnerRepresentation::Handle {
            revision: ContentHash::of_bytes(b"stdout"),
            resource: false,
        },
    );
    let token = owner(
        &mut owners,
        "Token",
        OwnerSpec {
            resource: true,
            finalizer: true,
            ..OwnerSpec::default()
        },
        |t| {
            let s = t.named(&path("Stdout"), vec![]);
            t.record(&[s])
        },
    );
    let mut b = LayoutBuilder::new(&x86(), &owners);
    let mut t = ty();
    let tok = t.named(&token, vec![]);
    let i64_ = t.p(Primitive::I64);
    let boxed = t.boxed(i64_);
    let body = path("main");
    let captures = [(tok, Passing::Borrow), (boxed, Passing::Take)];
    let env = b
        .environment(
            &t.store,
            &EnvironmentShape {
                definition: DefId::intern(&body),
                body: 1,
                captures: &captures,
                sizes: &[],
                movable: &[],
            },
        )
        .unwrap();
    let table = checked(b);
    let r = &table.layouts[env.0 as usize];
    let (view, owned) = (r.fields[0].offset, r.fields[1].offset);
    assert!(r.scan.contains(&ScanStep::Handle {
        offset: view,
        kind: HandleKind::Unknown,
        ty: None,
        obtains: None
    }));
    let d = descriptor(&table, env).unwrap();
    let steps: Vec<_> = d
        .steps
        .iter()
        .map(|s| (s.kind, s.offset, s.field))
        .collect();
    assert_eq!(steps, [(StepKind::ReleaseLast, owned, 1)]);

    let mut m = table.clone();
    m.layouts[env.0 as usize].drop.clear(); // the owned box never released
    assert!(matches!(
        descriptor(&m, env),
        Err(DescriptorError::Malformed(_))
    ));
}

#[test]
fn bounded_packed_values_reserve_their_maximum_inside_ordinary_storage() {
    use hirsel_core_contract::Passing;
    for target in [x86(), TargetSpec::aarch64_unknown_linux_gnu()] {
        let owners = Owners::default();
        let mut b = LayoutBuilder::new(&target, &owners);
        let mut t = ty();
        let word = t.p(Primitive::U64);
        let choice = t.variant(&[&[word], &[word, word]]);
        let packed = t.builtin(BuiltinType::Packed, vec![GenericArgument::Type(choice)]);
        let tuple = t.tuple(&[packed, word]);
        let record = t.record(&[packed, word]);
        let variant = t.variant(&[&[packed, word], &[]]);
        let inline = t.inline(tuple, 2);
        let tuple_list = t.list(tuple);
        let ids =
            [tuple, record, variant, inline, tuple_list].map(|ty| b.layout(&t.store, ty).unwrap());
        let definition = path("packed_captures");
        let captures = [
            (packed, Passing::Take),
            (packed, Passing::Borrow),
            (word, Passing::Take),
        ];
        let env = b
            .environment(
                &t.store,
                &EnvironmentShape {
                    definition: DefId::intern(&definition),
                    body: 0,
                    captures: &captures,
                    sizes: &[],
                    movable: &[],
                },
            )
            .unwrap();
        let movable = b
            .environment(
                &t.store,
                &EnvironmentShape {
                    definition: DefId::intern(&definition),
                    body: 1,
                    captures: &captures,
                    sizes: &[],
                    movable: &[0],
                },
            )
            .unwrap();
        let table = checked(b);
        for id in &ids[..2] {
            assert_eq!(fixed(rec(&table, *id)), 32);
            assert_eq!(offsets(rec(&table, *id)), [0, 24]);
        }
        assert_eq!(fixed(rec(&table, ids[2])), 40);
        assert_eq!(fixed(rec(&table, ids[3])), 64);
        let LayoutKind::Block { payload } = rec(&table, ids[4]).kind else {
            panic!("list")
        };
        assert_eq!(extent(&table, payload, &2u64.to_le_bytes()), Ok(80));
        assert_eq!(offsets(rec(&table, env)), [0, 24, 48]);
        assert_eq!(fixed(rec(&table, env)), 56);
        assert_eq!(offsets(rec(&table, movable)), [0, 24, 48, 56]);
        assert_eq!(fixed(rec(&table, movable)), 64);
        // A direct List is sequential (MEM-13); an Inline still has no sequential form.
        let list = t.list(packed);
        let mut b = LayoutBuilder::new(&target, &owners);
        let id = b.layout(&t.store, list).unwrap();
        let table = checked(b);
        let LayoutKind::Block { payload } = rec(&table, id).kind else {
            panic!("list")
        };
        assert!(matches!(
            rec(&table, payload).kind,
            LayoutKind::Sequential { .. }
        ));
        let inline = t.inline(packed, 2);
        assert!(matches!(
            LayoutBuilder::new(&target, &owners).layout(&t.store, inline),
            Err(LayoutError::Unavailable { .. })
        ));
    }
}

/// A borrowed cursor is noncopyable even though none of its fields owns a
/// holder. The control is rejected without the borrowed-field exception.
#[test]
fn a_borrowed_cursor_may_be_noncopyable_without_owned_holders() {
    let mut owners = Owners::default();
    let cursor = path("Cursor");
    owners.nominals.insert(
        DefId::intern(&cursor.clone()),
        OwnerRepresentation::Cursor {
            revision: ContentHash::of_parts("test-cursor", [b"v1".as_slice()]),
        },
    );
    let mut t = ty();
    let cursor_ty = t.named(&cursor, vec![]);
    let word = t.p(Primitive::U64);
    let plain = t.record(&[word]);
    let boxed = t.boxed(word);
    let counted = t.record(&[boxed]);
    let mut b = LayoutBuilder::new(&x86(), &owners);
    let c = b.layout(&t.store, cursor_ty).unwrap();
    let p = b.layout(&t.store, plain).unwrap();
    let h = b.layout(&t.store, counted).unwrap();
    let table = checked(b);
    assert_eq!(table.layouts[c.0 as usize].copy, CopyRecipe::NotCopyable);
    let mut bad = table.clone();
    bad.layouts[p.0 as usize].copy = CopyRecipe::NotCopyable;
    rejects(&bad, Obligation::Copy);
    let mut bad = table;
    bad.layouts[h.0 as usize].copy = CopyRecipe::Bits;
    rejects(&bad, Obligation::Copy);
}

#[test]
fn a_variant_owned_resource_field_is_distinct_from_a_borrowed_capture() {
    let mut owners = Owners::default();
    let resource = owner(
        &mut owners,
        "Trace",
        OwnerSpec {
            resource: true,
            finalizer: true,
            ..OwnerSpec::default()
        },
        |t| {
            let word = t.p(Primitive::U64);
            t.record(&[word])
        },
    );
    let mut t = ty();
    let trace = t.named(&resource, vec![]);
    let option = t.variant(&[&[], &[trace]]);
    let mut b = LayoutBuilder::new(&x86(), &owners);
    let variant = b.layout(&t.store, option).unwrap();
    let body = path("main");
    let captures = [(trace, hirsel_core_contract::Passing::Borrow)];
    let environment = b
        .environment(
            &t.store,
            &EnvironmentShape {
                definition: DefId::intern(&body),
                body: 1,
                captures: &captures,
                sizes: &[],
                movable: &[],
            },
        )
        .unwrap();
    let table = checked(b);
    let variant = rec(&table, variant);
    let field = &variant.tag.as_ref().unwrap().variants[1].fields[0];
    assert!(!field_is_borrowed(&table, variant, field));
    let environment = rec(&table, environment);
    assert!(field_is_borrowed(
        &table,
        environment,
        &environment.fields[0]
    ));
}

#[test]
fn byte_slices_and_source_byte_sequences_share_one_element_layout() {
    for target in TargetSpec::executed() {
        let owners = Owners::default();
        let mut b = LayoutBuilder::new(&target, &owners);
        let mut types = ty();
        let byte = types.p(Primitive::U8);
        let bytes = types.p(Primitive::Bytes);
        let text = types.p(Primitive::String);
        let list = types.add(TypeKind::Builtin {
            kind: BuiltinType::List,
            arguments: vec![GenericArgument::Type(byte)].into(),
        });
        let byte = b.layout(&types.store, byte).unwrap();
        for ty in [bytes, text] {
            let slice = b.layout(&types.store, ty).unwrap();
            assert_eq!(
                b.table().layouts[slice.0 as usize].kind,
                LayoutKind::Slice { element: byte }
            );
        }
        let list = b.layout(&types.store, list).unwrap();
        let LayoutKind::Block { payload } = b.table().layouts[list.0 as usize].kind else {
            panic!()
        };
        assert_eq!(
            b.table().layouts[payload.0 as usize].kind,
            LayoutKind::Sequence { element: byte }
        );
        assert!(validate_table(b.table(), &target).is_ok());
    }
}

#[test]
fn fixed_flat_values_and_elements_share_one_layout_in_every_request_order() {
    let mut owners = Owners::default();
    let fixed = owner(
        &mut owners,
        "FixedFlat",
        OwnerSpec {
            promises: Layout {
                flat: true,
                ..Layout::default()
            },
            ..OwnerSpec::default()
        },
        |t| {
            let small = t.p(Primitive::U8);
            let large = t.p(Primitive::U64);
            t.record(&[small, large])
        },
    );
    for first in 0..3 {
        let mut b = LayoutBuilder::new(&x86(), &owners);
        let mut t = ty();
        let ty = t.named(&fixed, vec![]);
        let boxed = t.boxed(ty);
        let list = t.list(ty);
        match first {
            0 => {
                b.layout(&t.store, ty).unwrap();
            }
            1 => {
                b.flat(&t.store, ty).unwrap();
            }
            _ => {
                b.layout(&t.store, boxed).unwrap();
            }
        }
        let memory = b.layout(&t.store, ty).unwrap();
        assert_eq!(memory, b.flat(&t.store, ty).unwrap());
        let boxed = b.layout(&t.store, boxed).unwrap();
        assert_eq!(b.record(boxed).kind, LayoutKind::Box { payload: memory });
        let list = b.layout(&t.store, list).unwrap();
        let LayoutKind::Block { payload } = b.record(list).kind else {
            panic!()
        };
        assert_eq!(
            b.record(payload).kind,
            LayoutKind::Sequence { element: memory }
        );
        assert_eq!(b.record(memory).size, SizeSpec::Fixed(9));
        checked(b);
    }
}

#[test]
fn a_flat_sequence_of_wide_scalars_marks_its_elements_unaligned() {
    let owners = Owners::default();
    let mut b = LayoutBuilder::new(&x86(), &owners);
    let mut t = ty();
    let scalar = t.p(Primitive::U64);
    let list = t.list(scalar);
    let encoding = b.flat(&t.store, list).unwrap();
    assert_eq!(b.record(encoding).align, 1);
    assert!(b.record(encoding).fields[0].unaligned);
    checked(b);
}

#[test]
fn ordinary_environment_data_preserves_nested_borrowed_fields() {
    use hirsel_core_contract::Passing;
    let owners = Owners::default();
    let mut builder = LayoutBuilder::new(&x86(), &owners);
    let mut types = ty();
    let text = types.p(Primitive::String);
    let text = builder.layout(&types.store, text).unwrap();
    let data = builder
        .environment_data(&path("producer"), 1, &[(text, Passing::Borrow)], &[])
        .unwrap();
    let wrapper = builder
        .environment_data(&path("producer"), 2, &[(data, Passing::Borrow)], &[])
        .unwrap();
    assert_eq!(
        builder
            .environment_data(&path("producer"), 1, &[(text, Passing::Borrow)], &[])
            .unwrap(),
        data
    );
    let table = checked(builder);
    for id in [data, wrapper] {
        let record = &table.layouts[id.0 as usize];
        assert_eq!(record.kind, LayoutKind::Aggregate);
        assert_eq!(record.scan, [ScanStep::Uncounted { offset: 0 }]);
        assert!(record.drop.is_empty());
        assert_eq!(record.copy, CopyRecipe::Bits);
        assert_eq!(
            hirsel_lir::layout::field_is_borrowed(&table, record, &record.fields[0]),
            id == data,
            "the wrapper carries an already uncounted ordinary record"
        );
    }
}

#[test]
fn a_borrow_without_runtime_holders_keeps_its_ordinary_field_copy_recipe() {
    use hirsel_core_contract::Passing;
    let mut owners = Owners::default();
    let token = owner(
        &mut owners,
        "Token",
        OwnerSpec {
            resource: true,
            ..OwnerSpec::default()
        },
        |t| t.p(Primitive::U64),
    );
    let mut types = ty();
    let token = types.named(&token, vec![]);
    let mut builder = LayoutBuilder::new(&x86(), &owners);
    let token = builder.layout(&types.store, token).unwrap();
    let data = builder
        .environment_data(&path("producer"), 1, &[(token, Passing::Borrow)], &[])
        .unwrap();
    let table = checked(builder);
    let record = &table.layouts[data.0 as usize];
    assert_eq!(record.copy, CopyRecipe::NotCopyable);
    assert!(record.drop.is_empty());
    assert!(record.scan.is_empty());
}

/// Runtime ABI 5: a sequence payload holds its u64 count at 0 and its u64 capacity at 8,
/// and its elements start past both at their alignment. C rejects a capacity word moved
/// into the elements or dropped.
#[test]
fn a_sequence_payload_holds_its_count_and_capacity_before_its_elements() {
    let owners = Owners::default();
    let mut b = LayoutBuilder::new(&x86(), &owners);
    let mut t = ty();
    let (byte, wide) = (t.p(Primitive::U8), t.p(Primitive::U128));
    let (bytes, wides) = (t.list(byte), t.list(wide));
    let bytes = b.layout(&t.store, bytes).unwrap();
    let wides = b.layout(&t.store, wides).unwrap();
    let table = checked(b);
    let payload = |id| match rec(&table, id).kind {
        LayoutKind::Block { payload } => payload,
        ref kind => panic!("a list is a block, not {kind:?}"),
    };
    for (list, first) in [(bytes, 16), (wides, 16)] {
        let r = rec(&table, payload(list));
        assert_eq!(offsets(r), [0, 8]);
        let SizeSpec::Dynamic(program) = &r.size else {
            panic!("a sequence has a dynamic size")
        };
        assert_eq!(program[0], SizeTerm::Constant(first));
    }
    let p = payload(bytes).0 as usize;
    let mut m = table.clone();
    m.layouts[p].fields[1].offset = 16;
    rejects(&m, Obligation::Offset);
    let mut m = table.clone();
    m.layouts[p].fields.pop();
    rejects(&m, Obligation::Offset);
}

/// C-facing nullable pointers use the same niche as Option<NonNull<T>> and
/// Option<extern fn> in Rust. A numeric word is still not a sound niche.
#[test]
fn foreign_pointer_options_use_one_word_without_nested_option_collapse() {
    for target in TargetSpec::executed() {
        let owners = Owners::default();
        let mut t = ty();
        let byte = t.p(Primitive::U8);
        let mut b = LayoutBuilder::new(&target, &owners);
        for kind in [BuiltinType::Ptr, BuiltinType::FnPtr] {
            let pointer = t.builtin(kind, vec![GenericArgument::Type(byte)]);
            let option = t.variant(&[&[], &[pointer]]);
            let nested = t.variant(&[&[], &[option]]);
            let pointer = b.layout(&t.store, pointer).unwrap();
            let option = b.layout(&t.store, option).unwrap();
            let nested = b.layout(&t.store, nested).unwrap();
            assert_eq!(b.record(option).size, SizeSpec::Fixed(8));
            assert_eq!(b.record(option).align, 8);
            assert!(matches!(
                b.record(option).tag.as_ref().unwrap().form,
                TagForm::Niche {
                    nullary: 0,
                    dataful: 1
                }
            ));
            assert_eq!(
                b.record(nested).tag.as_ref().unwrap().form,
                TagForm::Ordinal
            );
            assert_eq!(
                hirsel_lir::abi::foreign_scalar(b.table(), option),
                Some(ScalarKind::Pointer)
            );
            assert_eq!(
                hirsel_lir::abi::foreign_nonnull_offsets(b.table(), pointer),
                [0]
            );
            assert!(hirsel_lir::abi::foreign_nonnull_offsets(b.table(), option).is_empty());
        }
        checked(b);
    }
}

/// `Value = Null | Number(i64) | Items(List[Member])` with `Member { i64, Value }`:
/// a sequence reaches Member while Value is under way, so Member waits for it
/// as a Box payload would, whichever type is requested first.
#[test]
fn a_sequence_element_holding_its_owner_inline_waits_for_it() {
    let mut owners = Owners::default();
    let value = owner(&mut owners, "Value", OwnerSpec::default(), |t| {
        let n = t.p(Primitive::I64);
        let member = t.named(&path("Member"), vec![]);
        let items = t.list(member);
        t.variant(&[&[], &[n], &[items]])
    });
    let member = owner(&mut owners, "Member", OwnerSpec::default(), |t| {
        let n = t.p(Primitive::I64);
        let value = t.named(&path("Value"), vec![]);
        t.record(&[n, value])
    });
    let mut keys = vec![];
    for value_first in [true, false] {
        let mut b = LayoutBuilder::new(&x86(), &owners);
        let mut t = ty();
        let (v, m) = (t.named(&value, vec![]), t.named(&member, vec![]));
        let (vid, mid) = if value_first {
            let vid = b.layout(&t.store, v).unwrap();
            (vid, b.layout(&t.store, m).unwrap())
        } else {
            let mid = b.layout(&t.store, m).unwrap();
            (b.layout(&t.store, v).unwrap(), mid)
        };
        let table = checked(b);
        // Member holds Value inline after its i64.
        assert_eq!(rec(&table, mid).fields[1].layout, vid);
        keys.push((rec(&table, vid).key.clone(), rec(&table, mid).key.clone()));
    }
    assert_eq!(keys[0], keys[1]);
}

#[test]
fn strings_and_bytes_share_physical_layouts_in_every_request_order() {
    for target in TargetSpec::executed() {
        for reverse in [false, true] {
            let owners = Owners::default();
            let mut b = LayoutBuilder::new(&target, &owners);
            let mut types = ty();
            let text = types.p(Primitive::String);
            let bytes = types.p(Primitive::Bytes);
            assert_ne!(text, bytes);
            let text_list = types.list(text);
            let bytes_list = types.list(bytes);
            let text_record = types.record(&[text, text_list]);
            let bytes_record = types.record(&[bytes, bytes_list]);
            for (a, z) in [
                (text, bytes),
                (text_list, bytes_list),
                (text_record, bytes_record),
            ] {
                let (a, z) = if reverse { (z, a) } else { (a, z) };
                let first = b.layout(&types.store, a).unwrap();
                let second = b.layout(&types.store, z).unwrap();
                assert_eq!(first, second);
                assert_eq!(b.record(first).key, b.record(second).key);
                let instance = |ty| {
                    hirsel_layout::instance::InstanceKey::new(
                        &path("identity"),
                        0,
                        &[(
                            hirsel_layout::instance::ActualBinder::Type(TypeParameterRef::bound(
                                TypeParamId(0),
                            )),
                            GenericArgument::Type(ty),
                        )],
                        &types.store,
                        None,
                        None,
                    )
                    .unwrap()
                };
                assert_ne!(instance(a), instance(z));
            }
            checked(b);
        }
    }
}

/// Codec keys (design §1e): a codec plan names its layouts by content, so the plan of
/// one value over a table whose layouts are numbered apart, because unrelated layouts
/// were requested first, has the same `CodecKey` for text, boxes, sequences, maps, sets
/// and Path. Editing a layout the plan reaches changes the key.
#[test]
fn codec_keys_survive_unrelated_layouts() {
    use hirsel_lir::codec::{CodecNode, CodecNodeId, CodecPlan};
    use hirsel_lir::glue::GlueCodec;
    let owners = |set_index: Primitive| {
        let mut owners = Owners::default();
        let container = |name: &str, generics, index: Primitive| {
            representation(
                name,
                OwnerSpec {
                    generics,
                    ..OwnerSpec::default()
                },
                move |t| {
                    let (k, u64_, index) = (t.param(0), t.p(Primitive::U64), t.p(index));
                    let entry = if generics == 2 {
                        let v = t.param(1);
                        t.tuple(&[k, v])
                    } else {
                        k
                    };
                    let key = t.inline(u64_, 2);
                    let entries = t.list(entry);
                    let index = t.list(index);
                    t.record(&[key, entries, index])
                },
            )
        };
        owners
            .containers
            .insert(Container::Map, container("Map", 2, Primitive::U32));
        owners
            .containers
            .insert(Container::Set, container("Set", 1, set_index));
        owners.nominals.insert(
            DefId::intern(&DefPath::core("Path")),
            OwnerRepresentation::Handle {
                revision: ContentHash::of_bytes(b"Path"),
                resource: false,
            },
        );
        owners
    };
    let mut t = ty();
    let (u64_, string, bytes) = (
        t.p(Primitive::U64),
        t.p(Primitive::String),
        t.p(Primitive::Bytes),
    );
    let boxed = t.boxed(u64_);
    let list = t.list(u64_);
    let map = t.builtin(
        BuiltinType::Map,
        vec![GenericArgument::Type(string), GenericArgument::Type(u64_)],
    );
    let set = t.builtin(BuiltinType::Set, vec![GenericArgument::Type(u64_)]);
    let path_ = t.named(&DefPath::core("Path"), vec![]);
    let root = t.record(&[string, boxed, list, map, set, path_]);
    let (i16_, f32_) = (t.p(Primitive::I16), t.p(Primitive::F32));
    let other = t.list(f32_);
    let unrelated = [t.record(&[i16_, other]), other, t.boxed(i16_)];
    let key = |owners: &Owners, first: &[TypeId]| {
        let mut b = LayoutBuilder::new(&x86(), owners);
        for ty in first {
            b.layout(&t.store, *ty).unwrap();
        }
        let [
            u64_l,
            string_l,
            bytes_l,
            boxed_l,
            list_l,
            map_l,
            set_l,
            path_l,
            root_l,
        ] = [u64_, string, bytes, boxed, list, map, set, path_, root]
            .map(|ty| b.layout(&t.store, ty).unwrap());
        let table = checked(b);
        let u8_l = LayoutId(
            table
                .layouts
                .iter()
                .position(|l| l.kind == LayoutKind::Scalar(ScalarKind::U8))
                .expect("a byte sequence's element") as u32,
        );
        let storage = hirsel_lir::emission::slice_storage(&table, u8_l).expect("byte storage");
        let n = |i: u32| CodecNodeId(i);
        let nodes = vec![
            CodecNode::Scalar(u64_l),
            CodecNode::Text {
                layout: string_l,
                storage,
                utf8: true,
            },
            CodecNode::Box {
                layout: boxed_l,
                payload: n(0),
            },
            CodecNode::Sequence {
                layout: list_l,
                element: n(0),
            },
            CodecNode::Map {
                layout: map_l,
                key: n(1),
                value: n(0),
            },
            CodecNode::Set {
                layout: set_l,
                element: n(0),
            },
            CodecNode::Path(path_l),
            CodecNode::Fields {
                layout: root_l,
                fields: (1..7).map(n).collect(),
            },
            CodecNode::Text {
                layout: bytes_l,
                storage,
                utf8: false,
            },
        ];
        let plan = CodecPlan {
            descriptor: b"fixture schema".to_vec(),
            nodes,
            root: n(7),
            bytes: n(8),
            result: None,
            checks: Vec::new(),
        };
        let keys = hirsel_lir::LayoutKeys::new(&table);
        let codec = GlueCodec::of(&plan, &[], &table, &keys).unwrap();
        (codec.key, set_l)
    };
    let plain = owners(Primitive::U32);
    let (base, base_set) = key(&plain, &[]);
    let (shifted, shifted_set) = key(&plain, &unrelated);
    assert_ne!(
        base_set, shifted_set,
        "unrelated layouts renumber the table"
    );
    assert_eq!(base, shifted);
    let (edited, _) = key(&owners(Primitive::U16), &unrelated);
    assert_ne!(base, edited, "the Set's representation is in the key");
}

#[test]
fn movable_zero_sized_finalizers_keep_distinct_declaration_guards_at_equal_offsets() {
    use hirsel_core_contract::Passing;
    let mut owners = Owners::default();
    let token = owner(
        &mut owners,
        "EmptyToken",
        OwnerSpec {
            resource: true,
            finalizer: true,
            ..OwnerSpec::default()
        },
        |types| types.record(&[]),
    );
    let mut types = ty();
    let token_type = types.named(&token, vec![]);
    for target in [x86(), TargetSpec::aarch64_unknown_linux_gnu()] {
        let mut builder = LayoutBuilder::new(&target, &owners);
        let token = builder.layout(&types.store, token_type).unwrap();
        assert_eq!(fixed(builder.record(token)), 0);
        let environment = builder
            .environment_data(
                &path("zero_captures"),
                1,
                &[(token, Passing::Take), (token, Passing::Take)],
                &[0, 1],
            )
            .unwrap();
        let table = checked(builder);
        let record = rec(&table, environment);
        assert_eq!(record.fields[0].offset, record.fields[1].offset);
        let live = record.fields[2].offset;
        let guard = |bit| DropStep::ActiveVariant {
            tag_offset: live,
            form: TagForm::LiveBit { bit },
            variants: vec![(
                1,
                vec![DropStep::Child {
                    offset: 0,
                    layout: token,
                }],
            )],
        };
        assert_eq!(record.drop, vec![guard(1), guard(0)]);
        for removed in 0..2 {
            let mut missing = table.clone();
            missing.layouts[environment.0 as usize].drop.remove(removed);
            rejects(&missing, Obligation::DropOrder);
        }
        let mut swapped = table.clone();
        let DropStep::ActiveVariant { form, .. } =
            &mut swapped.layouts[environment.0 as usize].drop[0]
        else {
            unreachable!()
        };
        *form = TagForm::LiveBit { bit: 0 };
        rejects(&swapped, Obligation::DropOrder);
    }
}
