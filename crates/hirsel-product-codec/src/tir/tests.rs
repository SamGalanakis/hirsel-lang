use super::*;
use hirsel_artifact_store::{Contents, Domain, Memory};
use hirsel_types::{Type, TypeInterner, TypeKind, UseKind};
const DOMAIN: Domain = CheckedTir::DOMAIN;
fn fixture(reverse: bool) -> (Arc<TypeRecords>, Definition, Anchor) {
    let store = Arc::new(Memory::default());
    let types = TypeInterner::new(None);
    if reverse {
        types.intern(Type::with_kind(TypeKind::Error));
    }
    let unit = types.intern(Type::unit());
    let path = DefPath::core("archive-main");
    let anchor = Anchor::fresh();
    let span = Span::new(anchor, 1, 5).unwrap();
    let body = Body {
        arena_scope_grant: None,
        parameter_scope: hirsel_syntax::LexicalOwner::first(&path)
            .site(hirsel_syntax::NameSite::Item),
        locals: vec![Local {
            ty: unit,
            storage: Storage::ReturnSlot,
            scope: ScopeId(0),
            span,
        }],
        places: vec![Place {
            loan: None,
            binding: None,
            root: LocalId(0),
            projection: vec![].into(),
            ty: unit,
        }],
        scopes: vec![Scope { parent: None, span }],
        blocks: vec![Block {
            instructions: vec![Instruction {
                kind: InstructionKind::Assign {
                    destination: PlaceId(0),
                    value: RvalueId(0),
                },
                span,
                on_failure: CleanupId(0),
            }],
            terminator: Terminator::Return {
                value: Operand {
                    place: PlaceId(0),
                    usage: UseKind::Take,
                },
                cleanup: CleanupId(0),
            },
            span,
        }],
        rvalues: vec![Rvalue::Constant(Constant::Unit)],
        cleanups: vec![Cleanup::default()],
        cleanup_nodes: vec![],
        entry: BlockId(0),
        parameters: vec![],
        result: unit,
        borrow_cursors: vec![],
        tail_sites: vec![],
        matches: vec![],
        scoped_parameters: vec![],
    };
    (
        TypeRecords::new(types, store),
        Definition {
            inferred_unique: Default::default(),
            caller_bodies: vec![],
            protocol_associations: vec![],
            types: vec![unit].into(),
            path: DefId::intern(&path),
            signature_hash: ContentHash::default(),
            bodies: vec![body],
            entry: BodyId(0),
            contracts: vec![],
        },
        anchor,
    )
}
fn bytes(records: &TypeRecords, definition: &Definition, anchor: Anchor) -> Vec<u8> {
    records
        .encode_tir(definition, |a| {
            (a == anchor).then(|| (definition.path.path().clone(), 0))
        })
        .unwrap()
}
fn open(bytes: Vec<u8>, records: Arc<TypeRecords>, anchor: Anchor) -> Option<CheckedTir> {
    let expected = DOMAIN.reference(&bytes)?;
    CheckedTir::open(bytes.into(), expected, records, |_, _| Some(anchor))
}
#[test]
fn indexed_tir_rows_roundtrip_and_rebind_without_materializing_a_body() {
    let (records, definition, anchor) = fixture(false);
    let encoded = bytes(&records, &definition, anchor);
    let current = Anchor::fresh();
    let reader = open(encoded, records.clone(), current).unwrap();
    assert_eq!(reader.header().unwrap().path, definition.path);
    assert_eq!(reader.body_count(), 1);
    assert!(reader.body(1).is_none());
    assert_eq!(reader.type_count(), 1);
    let child = reader.type_reference(0).unwrap();
    assert!(records.open_component(child).is_some());
    let body = reader.body(0).unwrap();
    assert_eq!(body.local_count(), 1);
    assert!(body.local(1).is_none());
    assert_eq!(body.local(0).unwrap().span.anchor, current);
    assert_eq!(body.rvalue(0), Some(Rvalue::Constant(Constant::Unit)));
    let block = body.block(0).unwrap();
    assert_eq!(block.instruction_count(), 1);
    assert!(matches!(
        block.instruction(0).unwrap().kind,
        InstructionKind::Assign { .. }
    ));
    assert!(block.instruction(1).is_none());
    assert_eq!(block.span().unwrap().anchor, current);
    let mut expected = definition.clone();
    expected.bodies[0].locals[0].span.anchor = current;
    expected.bodies[0].scopes[0].span.anchor = current;
    expected.bodies[0].blocks[0].span.anchor = current;
    expected.bodies[0].blocks[0].instructions[0].span.anchor = current;
    assert_eq!(reader.materialize(), Some(expected));
}
#[test]
fn indexed_tir_canonical_bytes_ignore_pool_order_and_prior_root_requests() {
    let (a, da, aa) = fixture(false);
    let (b, db, ab) = fixture(true);
    b.encode(&[db.bodies[0].result]).unwrap();
    assert_eq!(bytes(&a, &da, aa), bytes(&b, &db, ab));
}
#[test]
fn indexed_tir_normalizes_equivalent_source_sharing() {
    let (records, mut first, anchor) = fixture(false);
    first.bodies.push(first.bodies[0].clone());
    let mut split = first.clone();
    let other = Anchor::fresh();
    split.bodies[1].locals[0].span.anchor = other;
    split.bodies[1].scopes[0].span.anchor = other;
    split.bodies[1].blocks[0].span.anchor = other;
    split.bodies[1].blocks[0].instructions[0].span.anchor = other;
    let split = records
        .encode_tir(&split, |a| {
            [anchor, other]
                .contains(&a)
                .then(|| (first.path.path().clone(), 0))
        })
        .unwrap();
    assert_eq!(bytes(&records, &first, anchor), split);
}
#[test]
fn checked_tir_rejects_unused_or_duplicate_references_and_wrong_scalar_versions() {
    let (records, definition, anchor) = fixture(false);
    let encoded = bytes(&records, &definition, anchor);
    let envelope =
        IndexedEnvelope::open(encoded.clone().into(), DOMAIN.reference(&encoded).unwrap()).unwrap();
    let owners: Vec<_> = (0..envelope.anchor_count())
        .map(|n| envelope.anchor(n).unwrap().to_owned())
        .collect();
    let mut references: Vec<_> = (0..envelope.type_count())
        .map(|n| envelope.type_reference(n).unwrap())
        .collect();
    references.push(references[0]);
    let bad = IndexedEnvelope::encode(&owners, &references, &envelope.payload()).unwrap();
    assert!(open(bad, records.clone(), anchor).is_none());
    let mut payload = envelope.payload().to_vec();
    let header = number(&payload, HEADER).unwrap();
    payload[header..header + 4].copy_from_slice(&u32::MAX.to_le_bytes());
    let references = &references[..references.len() - 1];
    let bad = IndexedEnvelope::encode(&owners, references, &payload).unwrap();
    assert!(open(bad, records.clone(), anchor).is_none());
    payload = envelope.payload().to_vec();
    let types = number(&payload, HEADER + 3 * EXTENT).unwrap();
    let ty = types + number(&payload[types..], HEADER).unwrap();
    payload[ty..ty + 4].copy_from_slice(&u32::MAX.to_le_bytes());
    let bad = IndexedEnvelope::encode(&owners, references, &payload).unwrap();
    assert!(open(bad, records, anchor).is_none());
}
#[test]
fn indexed_tir_reads_check_each_child_and_keep_full_backing_after_provider_drop() {
    use std::sync::atomic::{AtomicBool, Ordering};
    struct Lease(Arc<AtomicBool>);
    impl Drop for Lease {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }
    let (records, definition, anchor) = fixture(false);
    let encoded = bytes(&records, &definition, anchor);
    let expected = DOMAIN.reference(&encoded).unwrap();
    let released = Arc::new(AtomicBool::new(false));
    let data = Bytes::from(encoded).retain_owner(Arc::new(Lease(released.clone())));
    let mut parent = Vec::new();
    data.visit_retained_allocations(|id, n| parent.push((id, n)));
    let missing = TypeRecords::new(TypeInterner::new(None), Arc::new(Memory::default()));
    let reader = CheckedTir::open(data, expected, missing, |_, _| Some(anchor)).unwrap();
    assert_eq!(reader.header().unwrap().path, definition.path);
    assert!(
        reader.body(0).unwrap().header().is_none(),
        "the parent SHA cannot supply the result's type"
    );
    assert!(reader.body(0).unwrap().local(0).is_none());
    // Rows without a type reference are independently readable.
    assert!(
        reader
            .body(0)
            .unwrap()
            .block(0)
            .unwrap()
            .instruction(0)
            .is_some()
    );
    let child = reader.payload.range(0, 8, 1).unwrap();
    let mut retained = Vec::new();
    child.visit_retained_allocations(|id, n| retained.push((id, n)));
    assert_eq!(parent, retained);
    drop((reader, records));
    assert!(!released.load(Ordering::SeqCst));
    assert_eq!(&*child, MAGIC);
    drop(child);
    assert!(released.load(Ordering::SeqCst));
}
#[test]
fn checksummed_tir_tables_reject_wraparound_wrong_tags_counts_overlap_and_truncation() {
    let (records, definition, anchor) = fixture(false);
    let encoded = bytes(&records, &definition, anchor);
    let envelope =
        IndexedEnvelope::open(encoded.clone().into(), DOMAIN.reference(&encoded).unwrap()).unwrap();
    let start = envelope.payload().as_ptr() as usize - envelope.backing().as_ptr() as usize;
    let root = &encoded[start..];
    let bodies_start = number(root, HEADER + 4 * EXTENT).unwrap();
    let body_start = bodies_start + number(&root[bodies_start..], HEADER).unwrap();
    for fault in 0..8 {
        let mut bad = encoded.clone();
        let root = &mut bad[start..];
        match fault {
            0 => root[8..12].copy_from_slice(&u32::MAX.to_le_bytes()),
            1 => root[12..16].copy_from_slice(&u32::MAX.to_le_bytes()),
            2 => root[HEADER..HEADER + 8].copy_from_slice(&u64::MAX.to_le_bytes()),
            3 => root[HEADER + 8..HEADER + 16].copy_from_slice(&u64::MAX.to_le_bytes()),
            4 => root[HEADER + EXTENT..HEADER + EXTENT + 8].copy_from_slice(&0u64.to_le_bytes()),
            5 => root[bodies_start + 8..bodies_start + 12].copy_from_slice(&BLOCKS.to_le_bytes()),
            6 => root[body_start + 12..body_start + 16].copy_from_slice(&12u32.to_le_bytes()),
            7 => {
                let at = body_start + HEADER;
                root[at..at + 8].copy_from_slice(&u64::MAX.to_le_bytes());
            }
            _ => unreachable!(),
        }
        assert!(
            open(bad, records.clone(), anchor).is_none(),
            "fault {fault}"
        );
    }
    for end in 0..encoded.len() {
        assert!(open(encoded[..end].to_vec(), records.clone(), anchor).is_none());
    }
    let mut wrong = DOMAIN.reference(&encoded).unwrap();
    wrong.domain.schema += 1;
    assert!(CheckedTir::open(encoded.into(), wrong, records, |_, _| Some(anchor)).is_none());
}
