//! Descriptor controls; typed transition/CFG adversaries are separate.
use crate::support;
use hirsel_layout::layout::{LayoutBuilder, Owners};
use hirsel_lir::logical_storage::LogicalBytes;
use hirsel_lir::operation::{NativeType, OperationArgument, OperationDeclarations};
use hirsel_lir::{LayoutTable, TargetSpec};
use hirsel_types::*;

fn fixture() -> (
    hirsel_layout::operation::OperationRegistry,
    LayoutTable,
    hirsel_lir::sealed::SealDescriptor,
) {
    let types = support::session();
    let bytes = types.intern(Type::with_kind(TypeKind::Primitive(Primitive::Bytes)));
    let list = types.intern(Type::with_kind(TypeKind::Builtin {
        kind: BuiltinType::List,
        arguments: vec![GenericArgument::Type(bytes)].into(),
    }));
    let target = TargetSpec::x86_64_unknown_linux_gnu();
    let owners = Owners::default();
    let mut builder = LayoutBuilder::new(&target, &owners);
    builder.layout(&types, list).unwrap();
    let table = builder.finish();
    let arguments =
        hirsel_layout::operation::collect_arguments(&types, &[GenericArgument::Type(list)])
            .unwrap();
    let [OperationArgument::Type(root)] = arguments.as_slice() else {
        unreachable!()
    };
    let registry = support::registry();
    let descriptor = registry.seal_descriptor(root, &table, &target).unwrap();
    (registry, table, descriptor)
}

use hirsel_core_contract::{OperationId, Passing};
use hirsel_lir::ir::*;
use hirsel_lir::layout_types::{LayoutId, ScalarKind};
use hirsel_lir::sealed::{SourceEvent, SourceEventKind};
use hirsel_lir::stage::Rc as TestStage;

struct Declarations {
    registry: hirsel_layout::operation::OperationRegistry,
    expected: Option<Vec<SourceEvent>>,
    payloads: Option<Vec<hirsel_lir::sealed_payload_source::SourcePayload>>,
}
impl OperationDeclarations for Declarations {
    fn derive(
        &self,
        operation: &OperationId,
        arguments: &[OperationArgument],
        target: &TargetSpec,
    ) -> Result<(hirsel_lir::FnAbi, LayoutTable), hirsel_lir::layout::LayoutError> {
        self.registry.derive(operation, arguments, target)
    }
    fn seal_descriptor(
        &self,
        root: &NativeType,
        table: &LayoutTable,
        target: &TargetSpec,
    ) -> Result<hirsel_lir::sealed::SealDescriptor, hirsel_lir::layout::LayoutError> {
        self.registry.seal_descriptor(root, table, target)
    }
    fn seal_descriptor_for_source(
        &self,
        _: &hirsel_syntax::DefPath,
        _: hirsel_syntax::ContentHash,
        root: &NativeType,
        table: &LayoutTable,
        target: &TargetSpec,
    ) -> Result<hirsel_lir::sealed::SealDescriptor, hirsel_lir::layout::LayoutError> {
        self.registry.seal_descriptor(root, table, target)
    }
    fn seal_payloads(
        &self,
        _: &hirsel_syntax::DefPath,
        _: hirsel_syntax::ContentHash,
    ) -> Option<Vec<hirsel_lir::sealed_payload_source::SourcePayload>> {
        self.payloads.clone()
    }
    fn seal_events(
        &self,
        _: &hirsel_syntax::DefPath,
        _: hirsel_syntax::ContentHash,
    ) -> Option<Vec<SourceEvent>> {
        self.expected.clone()
    }
}

/// Original source correspondence is authored independently of the candidate's
/// instructions. Mutations never recompute it from the mutated LIR.
fn transition_fixture() -> (support::Lir<TestStage>, Declarations, ValueId, ValueId) {
    transition_fixture_with_root(false)
}

fn transition_fixture_with_root(
    heap: bool,
) -> (support::Lir<TestStage>, Declarations, ValueId, ValueId) {
    let types = support::session();
    let scalar = types.intern(Type::with_kind(TypeKind::Primitive(Primitive::U64)));
    let word = if heap {
        types.intern(Type::with_kind(TypeKind::Builtin {
            kind: BuiltinType::List,
            arguments: vec![GenericArgument::Type(scalar)].into(),
        }))
    } else {
        scalar
    };
    let region = types.intern(Type::with_kind(TypeKind::Builtin {
        kind: BuiltinType::Region,
        arguments: vec![GenericArgument::Type(word)].into(),
    }));
    let owners = Owners::default();
    let target = TargetSpec::x86_64_unknown_linux_gnu();
    let mut builder = LayoutBuilder::new(&target, &owners);
    let root_layout = builder.layout(&types, word).unwrap();
    let owner_layout = builder.layout(&types, region).unwrap();
    let extra = builder.finish();
    let mut t = support::Lir::<TestStage>::new(&[], None);
    let offset = t.table.layouts.len() as u32;
    for mut row in extra.layouts {
        row.for_each_reference_mut(&mut |id| id.0 += offset);
        t.table.layouts.push(row);
    }
    let root_layout = LayoutId(root_layout.0 + offset);
    let owner_layout = LayoutId(owner_layout.0 + offset);
    let abi = t.abi(
        &[
            (Passing::Take, support::l::ARENA),
            (Passing::Take, root_layout),
        ],
        Some(owner_layout),
    );
    t.builder.unit.header.abi = abi;
    t.builder.unit.header.origin = UnitOrigin::Library;
    let grant = t.plain(support::l::ARENA);
    let root = t.value_as(
        if heap {
            ValueRepr::Block
        } else {
            ValueRepr::Scalar(ScalarKind::U64)
        },
        root_layout,
        if heap { Holding::Owned } else { Holding::Plain },
    );
    t.param(grant);
    t.param(root);
    let owner = t.value_as(ValueRepr::Handle, owner_layout, Holding::Owned);
    let args = hirsel_layout::operation::collect_arguments(&types, &[GenericArgument::Type(word)])
        .unwrap();
    let [OperationArgument::Type(native)] = args.as_slice() else {
        unreachable!()
    };
    let native = native.clone();
    let descriptor = t
        .operations
        .seal_descriptor(&native, &t.table, &target)
        .unwrap();
    t.builder.unit.tables.seal_descriptors.push(descriptor);
    let failure = if heap {
        let cleanup = t.releasing(&[root]);
        t.failure(cleanup)
    } else {
        t.fails()
    };
    // An ordinary bit read of the token slot is a new SSA name of the
    // same grant. Consumption must follow that origin, including on CFG edges.
    let slot = t.slot(support::l::ARENA, SlotKind::Parameter(0), support::ROOT);
    let abort = if heap {
        vec![support::Undo::Release(root)]
    } else {
        vec![]
    };
    let address = t.at(support::ENTRY, slot, &[], &abort);
    let alias = t.read(support::ENTRY, address, grant, ReadMode::Bits, None);
    if heap {
        // Keep the ordinary cleanup independently reachable when a mutation
        // redirects only the seal's abort edge past it.
        let condition = t.boolean(support::ENTRY);
        let cleanup_edge = t.builder.unit.tables.failures[failure.index()].cleanup;
        let cleanup = t.builder.unit.tables.edges[cleanup_edge.index()].target;
        let check_failure = t.failure(cleanup);
        t.builder.push(
            support::ENTRY,
            Inst::Check {
                condition,
                failure: check_failure,
            },
        );
    }
    t.builder.push(
        support::ENTRY,
        Inst::SealArena {
            result: owner,
            grant: alias,
            root,
            shape: SealDescriptorId(0),
            failure,
            origin: SourceAnchorId(0),
        },
    );
    t.builder.ret(support::ENTRY, Some(owner));
    let declarations = Declarations {
        registry: t.operations.clone(),
        payloads: None,
        expected: Some(vec![SourceEvent {
            anchor: t.builder.unit.tables.source_anchors[0].clone(),
            kind: SourceEventKind::Seal,
            root_type: native,
        }]),
    };
    (t, declarations, alias, root)
}

fn checked(
    t: support::Lir<TestStage>,
    declarations: &Declarations,
) -> Vec<hirsel_lir::validate::Violation> {
    let (unit, table, manifest) = t.finish();
    checked_raw(unit, &table, &manifest, declarations)
}

fn checked_raw<S: hirsel_lir::stage::Stage>(
    unit: hirsel_lir::LirUnit<S>,
    table: &LayoutTable,
    manifest: &hirsel_core_contract::Manifest,
    declarations: &Declarations,
) -> Vec<hirsel_lir::validate::Violation> {
    let target = TargetSpec::x86_64_unknown_linux_gnu();
    let inputs = hirsel_lir::validate::Inputs {
        target: &target,
        layouts: &table,
        manifest: &manifest,
        operations: declarations,
        discharge: None,
        memo: Default::default(),
    };
    hirsel_lir::validate::validate(unit, &inputs)
        .err()
        .map_or(Vec::new(), |rejected| rejected.violations)
}

#[test]
fn ready_sealed_bits_33_keeps_source_and_descriptor_obligations() {
    let (t, declarations, _, _) = transition_fixture();
    let (unit, table, manifest) = t.finish();
    let unit = unit.into_stage::<hirsel_lir::stage::Ready>();
    assert_eq!(
        checked_raw(unit.clone(), &table, &manifest, &declarations),
        []
    );
    let mut borrowed = table.clone();
    let parameter = &mut borrowed.fn_abis[unit.header.abi.0 as usize].parameters[0];
    parameter.convention = Passing::Borrow;
    parameter.passing = hirsel_lir::abi_types::ParameterPassing::ReadOnlyAddress;
    sealed_rejection(
        &checked_raw(unit.clone(), &borrowed, &manifest, &declarations),
        "borrow-only",
    );
    let mut missing = declarations;
    missing.expected = None;
    sealed_rejection(
        &checked_raw(unit.clone(), &table, &manifest, &missing),
        "source event correspondence",
    );
    let (_, declarations, _, _) = transition_fixture();
    let mut forged = unit;
    forged.tables.seal_descriptors[0].logical.facts[0].bytes = LogicalBytes::Bytes(9);
    assert!(!checked_raw(forged, &table, &manifest, &declarations).is_empty());
}

#[test]
fn ready_sealed_bits_33_refuses_heap_roots_before_emission() {
    let (t, declarations, _, _) = transition_fixture_with_root(true);
    let (unit, table, manifest) = t.finish();
    sealed_rejection(
        &checked_raw(
            unit.into_stage::<hirsel_lir::stage::Ready>(),
            &table,
            &manifest,
            &declarations,
        ),
        "original pointer-free root",
    );
    let (t, declarations, _, _, _) = root_view_fixture(ViewRoot::Heap);
    let (unit, table, manifest) = t.finish();
    sealed_rejection(
        &checked_raw(
            unit.into_stage::<hirsel_lir::stage::Ready>(),
            &table,
            &manifest,
            &declarations,
        ),
        "original pointer-free root",
    );
}

#[test]
fn ready_sealed_bits_33_root_view_keeps_exclusive_owner_correspondence() {
    for kind in [ViewRoot::Unit, ViewRoot::Scalar] {
        let (t, declarations, _, _, other_owner) = root_view_fixture(kind);
        let (unit, table, manifest) = t.finish();
        let mut unit = unit.into_stage::<hirsel_lir::stage::Ready>();
        assert_eq!(
            checked_raw(unit.clone(), &table, &manifest, &declarations),
            []
        );
        for inst in &mut unit.instructions {
            if let Inst::BorrowSealedRoot { owner, .. } = inst {
                *owner = other_owner;
            }
        }
        sealed_rejection(
            &checked_raw(unit, &table, &manifest, &declarations),
            "exclusive owner loan",
        );
    }
}

#[test]
fn public_validation_accepts_a_declared_typed_seal_transition() {
    let (t, d, _, _) = transition_fixture();
    assert_eq!(checked(t, &d), []);
}

#[test]
fn source_correspondence_survives_candidate_event_deletion() {
    let (_, d, _, _) = transition_fixture();
    // A fully well-formed ordinary unit represents deletion of both producer
    // and sink, including every candidate descriptor. Its current-source
    // expectation still names the original seal operation independently.
    let mut missing = support::Lir::<TestStage>::new(&[], None);
    missing.builder.ret(support::ENTRY, None);
    let (unit, table, manifest) = missing.finish();
    let empty = Declarations {
        registry: d.registry.clone(),
        payloads: None,
        expected: Some(vec![]),
    };
    assert!(checked_raw(unit.clone(), &table, &manifest, &empty).is_empty());
    let errors = checked_raw(unit, &table, &manifest, &d);
    assert_eq!(errors.len(), 1, "{errors:?}");
    sealed_rejection(&errors, "source event correspondence");
    let (t, mut d, _, _) = transition_fixture();
    d.expected = None;
    assert!(!checked(t, &d).is_empty());
}

#[test]
fn public_validation_rejects_descriptor_charge_and_edge_mutations() {
    for mutation in 0..3 {
        let (mut t, d, _, _) = transition_fixture();
        let descriptor = &mut t.builder.unit.tables.seal_descriptors[0];
        match mutation {
            0 => descriptor.logical.facts[0].bytes = LogicalBytes::Bytes(9),
            1 => {
                descriptor.access.pop();
            }
            _ => {
                descriptor.root_type = NativeType::Primitive(hirsel_lir::operation::Primitive::I64)
            }
        }
        assert!(!checked(t, &d).is_empty(), "mutation {mutation}");
    }
}

#[test]
fn region_drop_cannot_be_laundered_into_an_ordinary_rc_release() {
    let (mut t, d, _, _) = transition_fixture();
    let root = t.builder.unit.tables.seal_descriptors[0].root_layout;
    let owner = t
        .table
        .layouts
        .iter_mut()
        .find(|row| row.kind == (hirsel_lir::layout_types::LayoutKind::SealedRegion { root }))
        .unwrap();
    owner.drop = vec![hirsel_lir::layout_types::DropStep::Release { offset: 0 }];
    assert!(!checked(t, &d).is_empty());
}

fn sealed_rejection(errors: &[hirsel_lir::validate::Violation], text: &str) {
    assert!(
        errors.iter().any(|e| matches!(&e.kind,
        hirsel_lir::validate::ViolationKind::Sealed(reason) if reason.contains(text))),
        "expected sealed rejection containing {text:?}, got {errors:?}"
    );
}

#[test]
fn ordinary_borrow_entry_cannot_forge_a_consumable_grant() {
    let (t, d, _, _) = transition_fixture();
    let (unit, mut table, manifest) = t.finish();
    assert_eq!(checked_raw(unit.clone(), &table, &manifest, &d), []);
    let parameter = &mut table.fn_abis[unit.header.abi.0 as usize].parameters[0];
    parameter.convention = Passing::Borrow;
    parameter.passing = hirsel_lir::abi_types::ParameterPassing::ReadOnlyAddress;
    sealed_rejection(&checked_raw(unit, &table, &manifest, &d), "borrow-only");
}

#[test]
fn duplicate_publication_through_a_plain_token_alias_is_rejected() {
    let (control, declarations, _, _) = transition_fixture();
    assert_eq!(checked(control, &declarations), []);
    let (mut t, mut d, grant, root) = transition_fixture();
    let owner = t
        .builder
        .unit
        .values
        .iter()
        .enumerate()
        .find(|(_, v)| v.holding == Holding::Owned)
        .map(|(i, _)| ValueId(i as u32))
        .unwrap();
    t.release(support::ENTRY, owner, &[]);
    let other = t.value_as(
        ValueRepr::Handle,
        t.builder.unit.types[t.builder.unit.values[owner.index()].ty.index()].layout,
        Holding::Owned,
    );
    let failure = t.fails();
    t.builder.push(
        support::ENTRY,
        Inst::SealArena {
            result: other,
            grant,
            root,
            shape: SealDescriptorId(0),
            failure,
            origin: SourceAnchorId(0),
        },
    );
    t.builder.ret(support::ENTRY, Some(other));
    let event = d.expected.as_ref().unwrap()[0].clone();
    d.expected.as_mut().unwrap().push(event);
    sealed_rejection(&checked(t, &d), "consumed");
}

#[test]
fn seal_failure_retains_the_owned_heap_root_for_compiler_cleanup() {
    let (t, d, _, root) = transition_fixture_with_root(true);
    let (unit, table, manifest) = t.finish();
    assert_eq!(checked_raw(unit.clone(), &table, &manifest, &d), []);
    let mut mutant = unit;
    // Redirect the seal abort edge past root cleanup. The old cleanup
    // remains well-formed; no dangling edge masks the ownership violation.
    let failure = mutant
        .instructions
        .iter()
        .find_map(|inst| match *inst {
            Inst::SealArena { failure, .. } => Some(failure),
            _ => None,
        })
        .unwrap();
    let bare_propagation = mutant
        .blocks
        .iter()
        .enumerate()
        .find_map(|(i, block)| {
            (block.kind == BlockKind::FailureCleanup
                && block.instructions.len == 0
                && matches!(mutant.terminators[i], Terminator::Propagate))
            .then_some(BlockId(i as u32))
        })
        .unwrap();
    let edge = mutant.tables.failures[failure.index()].cleanup;
    mutant.tables.edges[edge.index()].target = bare_propagation;
    let errors = checked_raw(mutant, &table, &manifest, &d);
    assert!(
        errors.iter().any(|e| matches!(e.kind,
        hirsel_lir::validate::ViolationKind::Leak(v) if v == root)),
        "{errors:?}"
    );
}

#[test]
fn branch_local_seal_consumption_survives_a_token_phi() {
    for mutant in [false, true] {
        let (mut t, mut d, grant, root) = transition_fixture();
        let owner = ValueId(2);
        let condition = t.boolean(support::ENTRY);
        let left = t.builder.block(BlockKind::Body, &[]);
        let right = t.builder.block(BlockKind::Body, &[]);
        let joined = t.plain(support::l::ARENA);
        let join = t.builder.block(BlockKind::Body, &[joined]);
        let yes = t.builder.edge(left, &[]);
        let no = t.builder.edge(right, &[]);
        t.builder
            .term(support::ENTRY, Terminator::Branch { condition, yes, no });
        t.builder.jump(left, join, &[grant]);
        t.builder.jump(right, join, &[grant]);
        if mutant {
            let another = t.value_as(
                ValueRepr::Handle,
                t.builder.unit.types[t.builder.unit.values[owner.index()].ty.index()].layout,
                Holding::Owned,
            );
            // First owner is dead on both arms before testing second publication.
            t.release(join, owner, &[]);
            let failure = t.fails();
            t.builder.push(
                join,
                Inst::SealArena {
                    result: another,
                    grant: joined,
                    root,
                    shape: SealDescriptorId(0),
                    failure,
                    origin: SourceAnchorId(0),
                },
            );
            t.builder.ret(join, Some(another));
            let event = d.expected.as_ref().unwrap()[0].clone();
            d.expected.as_mut().unwrap().push(event);
        } else {
            t.builder.ret(join, Some(owner));
        }
        let errors = checked(t, &d);
        if mutant {
            sealed_rejection(&errors, "consumed");
        } else {
            assert_eq!(errors, []);
        }
    }
}

#[derive(Clone, Debug, Default, Hash)]
struct LegacyTables {
    pub calls: Vec<CallSite>,
    pub constructs: Vec<ConstructSite>,
    pub closures: Vec<ClosureSite>,
    pub constants: Vec<ConstantData>,
    pub symbols: Vec<Symbol>,
    pub operations: Vec<OperationId>,
    pub loans: Vec<LoanInfo>,
    pub failures: Vec<FailureEdge>,
    pub edges: Vec<Edge>,
    pub masks: Vec<FieldMask>,
    pub regions: Vec<RegionInstance>,
    pub arenas: Vec<ExternalInstance>,
    pub shared: Vec<ExternalInstance>,
    pub task_domains: u32,
    pub source_anchors: Vec<SourceAnchor>,
    pub cleanup_anchors: Vec<CleanupAnchor>,
    /// Drop flags (LIR schema revision 8).
    pub init_flags: Vec<InitFlag>,
    /// SER-2 codec plans (LIR schema revision 29).
    pub codecs: Vec<hirsel_lir::codec::CodecPlan>,
    /// MEM-3 logical types and complete physical access, independently validated.
    pub structural: Vec<hirsel_lir::structural_access::AccessPlan>,
}
hirsel_codec::snapshot_struct!(LegacyTables {
    calls,
    constructs,
    closures,
    constants,
    symbols,
    operations,
    loans,
    failures,
    edges,
    masks,
    regions,
    arenas,
    shared,
    task_domains,
    source_anchors,
    cleanup_anchors,
    init_flags,
    codecs,
    structural
});

#[test]
fn absent_sealed_tables_preserve_ordinary_side_table_bytes_and_hash() {
    use std::hash::{Hash, Hasher};
    let old = LegacyTables {
        task_domains: 3,
        ..Default::default()
    };
    let value = SideTables {
        task_domains: 3,
        ..Default::default()
    };
    let old_text = hirsel_codec::to_text(&old).replace("LegacyTables", "SideTables");
    assert_eq!(hirsel_codec::to_text(&value), old_text);
    assert_eq!(
        hirsel_codec::from_text::<SideTables>(&old_text).unwrap(),
        value
    );
    let hash = |input: &dyn Fn(&mut std::collections::hash_map::DefaultHasher)| {
        let mut state = std::collections::hash_map::DefaultHasher::new();
        input(&mut state);
        state.finish()
    };
    assert_eq!(hash(&|s| old.hash(s)), hash(&|s| value.hash(s)));
    let (_, _, descriptor) = fixture();
    let demanded = SideTables {
        seal_descriptors: vec![descriptor],
        ..value.clone()
    };
    assert_ne!(hash(&|s| value.hash(s)), hash(&|s| demanded.hash(s)));
    let text = hirsel_codec::to_text(&demanded);
    assert_eq!(
        hirsel_codec::from_text::<SideTables>(&text).unwrap(),
        demanded
    );
}

#[test]
fn sealed_descriptor_objects_erase_and_rebind_complete_physical_projection() {
    let (t, _, _, _) = transition_fixture_with_root(true);
    let (unit, _, _) = t.finish();
    let descriptor = &unit.tables.seal_descriptors[0];
    let (erased, layouts, abis) = hirsel_lir::objects::erased(&unit);
    assert!(descriptor.layouts().all(|id| layouts.contains(&id)));
    assert!(layouts.len() > 1);
    let restored = hirsel_lir::objects::rebound(erased.clone(), &layouts, &abis).unwrap();
    assert_eq!(
        restored.tables.seal_descriptors,
        unit.tables.seal_descriptors
    );
    let moved: Vec<_> = layouts.iter().map(|id| LayoutId(id.0 + 100)).collect();
    let rebound = hirsel_lir::objects::rebound(erased, &moved, &abis).unwrap();
    let before: Vec<_> = descriptor.layouts().collect();
    let after: Vec<_> = rebound.tables.seal_descriptors[0].layouts().collect();
    assert_eq!(
        after,
        before
            .iter()
            .map(|id| LayoutId(id.0 + 100))
            .collect::<Vec<_>>()
    );
}

fn payload_fixture() -> (support::Lir<TestStage>, Declarations, ValueId) {
    use hirsel_lir::sealed_payload::*;
    let (mut t, mut declarations, _, root) = transition_fixture();
    let root_type = t.builder.unit.tables.seal_descriptors[0].root_type.clone();
    let recipe = PayloadRecipe {
        source: t.builder.unit.tables.source_anchors[0].clone(),
        operation: PayloadOperation::Transfer { previous: 1 },
        inputs: vec![PayloadOperand {
            place: 1,
            ty: root_type.clone(),
            producer: PayloadProducer::Parameter(1),
            capture_slot: None,
        }],
    };
    declarations.payloads = Some(vec![SourcePayload {
        root_type,
        recipe: recipe.clone(),
    }]);
    t.builder.unit.tables.seal_payloads.push(PayloadTable {
        descriptor: SealDescriptorId(0),
        bindings: vec![PayloadBinding {
            recipe,
            values: vec![root],
        }],
    });
    (t, declarations, root)
}

#[test]
fn semantic_payload_33_valid_source_recipe_binds_the_actual_operand_and_roundtrips() {
    let (t, declarations, _) = payload_fixture();
    let table = &t.builder.unit.tables.seal_payloads[0];
    let bytes = hirsel_codec::to_text(table);
    assert_eq!(
        hirsel_codec::from_text::<hirsel_lir::sealed_payload::PayloadTable>(&bytes).unwrap(),
        *table
    );
    assert!(checked(t, &declarations).is_empty());
}

#[test]
fn semantic_payload_33_forged_recipe_is_permanently_refused_by_public_c() {
    use hirsel_lir::sealed_payload::*;
    let (mut t, declarations, _) = payload_fixture();
    t.builder.unit.tables.seal_payloads[0].bindings[0]
        .recipe
        .operation = PayloadOperation::Allocate {
        destination: PayloadDestination::Heap,
    };
    sealed_rejection(
        &checked(t, &declarations),
        "recipes differ from independent source",
    );
}

#[test]
fn semantic_payload_33_operand_substitution_cannot_launder_source_lineage() {
    let (mut t, declarations, _) = payload_fixture();
    let grant = t.builder.unit.parameters[0];
    t.builder.unit.tables.seal_payloads[0].bindings[0].values[0] = grant;
    sealed_rejection(
        &checked(t, &declarations),
        "operand producer lineage differs",
    );
}

#[test]
fn semantic_payload_33_missing_recipe_cannot_delete_its_expected_source_obligation() {
    let (mut t, declarations, _) = payload_fixture();
    t.builder.unit.tables.seal_payloads.clear();
    sealed_rejection(
        &checked(t, &declarations),
        "recipes differ from independent source",
    );
}

#[test]
fn semantic_payload_33_unavailable_source_never_authorizes_candidate_recipes() {
    let (t, mut declarations, _) = payload_fixture();
    declarations.payloads = None;
    sealed_rejection(
        &checked(t, &declarations),
        "recipes differ from independent source",
    );
}

#[test]
fn semantic_payload_33_agreeing_false_recipe_claims_do_not_replace_ssa_proof() {
    use hirsel_lir::sealed_payload::*;
    let (mut t, mut declarations, _) = payload_fixture();
    // Even matching recipe/source declarations cannot assert that parameter 1
    // was produced by parameter 0. C follows the candidate value independently.
    declarations.payloads.as_mut().unwrap()[0].recipe.inputs[0].producer =
        PayloadProducer::Parameter(0);
    t.builder.unit.tables.seal_payloads[0].bindings[0]
        .recipe
        .inputs[0]
        .producer = PayloadProducer::Parameter(0);
    sealed_rejection(
        &checked(t, &declarations),
        "operand producer lineage differs",
    );
}

#[test]
fn semantic_payload_33_zero_sized_parameter_keeps_original_source_ordinal() {
    use hirsel_lir::sealed_payload::*;
    let (mut t, mut declarations, _) = payload_fixture();
    let old = t.builder.unit.header.abi.0 as usize;
    let owner = t.table.fn_abis[old].result.as_ref().unwrap().layout;
    let root = t.builder.unit.tables.seal_descriptors[0].root_layout;
    let unit = t.plain(support::l::UNIT);
    t.builder.unit.parameters.insert(1, unit);
    let abi = t.abi(
        &[
            (Passing::Take, support::l::ARENA),
            (Passing::Take, support::l::UNIT),
            (Passing::Take, root),
        ],
        Some(owner),
    );
    t.builder.unit.header.abi = abi;
    assert!(
        t.table.fn_abis[abi.0 as usize].parameters[1]
            .slots
            .is_empty()
    );
    declarations.payloads.as_mut().unwrap()[0].recipe.inputs[0].producer =
        PayloadProducer::Parameter(2);
    t.builder.unit.tables.seal_payloads[0].bindings[0]
        .recipe
        .inputs[0]
        .producer = PayloadProducer::Parameter(2);
    let errors = checked(t, &declarations);
    assert!(errors.is_empty(), "{errors:?}");
}

#[test]
fn receipt_local_33_public_c_refuses_producer_before_complete_custody_proof() {
    use hirsel_lir::layout_types::*;
    let (t, declarations, root) = payload_fixture();
    let (mut unit, mut table, manifest) = t.finish();
    let baseline = checked_raw(unit.clone(), &table, &manifest, &declarations);
    assert!(baseline.is_empty(), "{baseline:#?}");
    let cleanup = CleanupAnchorId(unit.tables.cleanup_anchors.len() as u32);
    unit.tables.cleanup_anchors.push(CleanupAnchor {
        body: 0,
        cleanup: INSTRUCTION_DROP,
        action: 0,
        site: unit.tables.source_anchors[0].site,
    });
    let inputs = PoolRange::push(
        &mut unit.pools.receipt_inputs,
        [ReceiptInput {
            original: root,
            receipt: None,
        }],
    );
    let mut row = table.layouts[0].clone();
    row.kind = LayoutKind::PayloadReceipt {
        root: unit.tables.seal_descriptors[0].root_type.clone(),
    };
    row.size = SizeSpec::Fixed(8);
    row.align = 8;
    row.fields.clear();
    row.tag = None;
    row.scan.clear();
    row.drop = vec![DropStep::ReleaseReceipt { offset: 0 }];
    row.copy = CopyRecipe::NotCopyable;
    row.finalizer = None;
    row.abi = AbiInputs {
        scalars: vec![(0, ScalarKind::Pointer)],
        unaligned: false,
    };
    let layout = LayoutId(table.layouts.len() as u32);
    table.layouts.push(row);
    let ty = LirTypeId(unit.types.len() as u32);
    unit.types.push(LirType {
        repr: ValueRepr::PayloadReceipt,
        layout,
    });
    let result = ValueId(unit.values.len() as u32);
    unit.values.push(ValueInfo {
        ty,
        holding: Holding::Owned,
    });
    unit.instructions.insert(
        1,
        Inst::PayloadReceipt {
            result,
            value: root,
            shape: SealDescriptorId(0),
            binding: PayloadBindingId(0),
            inputs,
        },
    );
    // Failure edges are owned by their operation, even for mechanical metadata.
    let mut release_failure = unit.tables.failures[0].clone();
    let release_cleanup = unit.tables.edges[release_failure.cleanup.index()].clone();
    release_failure.cleanup = EdgeId(unit.tables.edges.len() as u32);
    unit.tables.edges.push(release_cleanup);
    let failure = FailureId(unit.tables.failures.len() as u32);
    unit.tables.failures.push(release_failure);
    unit.instructions.insert(
        2,
        Inst::Release {
            value: result,
            layout,
            live: MaskRef::All,
            anchor: cleanup,
            failure,
        },
    );
    for (index, block) in unit.blocks.iter_mut().enumerate() {
        if index == support::ENTRY.index() {
            block.instructions.len += 2;
        } else {
            block.instructions.start += 2;
        }
    }
    let errors = checked_raw(unit.clone(), &table, &manifest, &declarations);
    assert!(
        errors.iter().any(
            |e| matches!(&e.kind, hirsel_lir::validate::ViolationKind::Schema(text)
        if text == "private receipt storage requires independent source and custody validation")
        ),
        "{errors:#?}"
    );
    let mut wrong = unit.clone();
    if let Inst::PayloadReceipt { binding, .. } = &mut wrong.instructions[1] {
        *binding = PayloadBindingId(1);
    }
    assert!(wrong.check_identity(&table).is_err());
    let mut wrong = unit;
    if let Inst::PayloadReceipt { inputs, .. } = &mut wrong.instructions[1] {
        *inputs = PoolRange::new(0, 2);
    }
    assert!(wrong.check_identity(&table).is_err());
}

#[derive(Clone, Copy, Debug)]
enum ViewRoot {
    Unit,
    Scalar,
    Heap,
}

/// Model region.with's two mut owner parameters independently of any seal
/// producer. The source expectation is fixed before candidate mutations.
fn root_view_fixture(
    kind: ViewRoot,
) -> (
    support::Lir<TestStage>,
    Declarations,
    ValueId,
    LoanId,
    ValueId,
) {
    let types = support::session();
    let scalar = types.intern(Type::with_kind(TypeKind::Primitive(Primitive::U64)));
    let root = match kind {
        ViewRoot::Unit => types.intern(Type::with_kind(TypeKind::Tuple(vec![].into()))),
        ViewRoot::Scalar => scalar,
        ViewRoot::Heap => types.intern(Type::with_kind(TypeKind::Builtin {
            kind: BuiltinType::List,
            arguments: vec![GenericArgument::Type(scalar)].into(),
        })),
    };
    let region = types.intern(Type::with_kind(TypeKind::Builtin {
        kind: BuiltinType::Region,
        arguments: vec![GenericArgument::Type(root)].into(),
    }));
    let target = TargetSpec::x86_64_unknown_linux_gnu();
    let owners = Owners::default();
    let mut layouts = LayoutBuilder::new(&target, &owners);
    let root_layout = layouts.layout(&types, root).unwrap();
    let owner_layout = layouts.layout(&types, region).unwrap();
    let mut t = support::Lir::<TestStage>::new(&[], None);
    let offset = t.table.layouts.len() as u32;
    for mut row in layouts.finish().layouts {
        row.for_each_reference_mut(&mut |id| id.0 += offset);
        t.table.layouts.push(row);
    }
    let root_layout = LayoutId(root_layout.0 + offset);
    let owner_layout = LayoutId(owner_layout.0 + offset);
    t.builder.unit.header.abi = t.abi(
        &[(Passing::Mut, owner_layout), (Passing::Mut, owner_layout)],
        None,
    );
    t.builder.unit.header.origin = UnitOrigin::Library;
    let arguments =
        hirsel_layout::operation::collect_arguments(&types, &[GenericArgument::Type(root)])
            .unwrap();
    let [OperationArgument::Type(native)] = arguments.as_slice() else {
        unreachable!()
    };
    t.builder.unit.tables.seal_descriptors.push(
        t.operations
            .seal_descriptor(native, &t.table, &target)
            .unwrap(),
    );
    let mut owner_views = Vec::new();
    for i in 0..2 {
        let ty = t.ty(ValueRepr::Handle, owner_layout);
        let slot = SlotId(t.builder.unit.slots.len() as u32);
        t.builder.unit.slots.push(SlotInfo {
            ty,
            kind: SlotKind::Parameter(i),
            scope: support::ROOT,
        });
        let version = t.value_as(ValueRepr::Handle, owner_layout, Holding::Plain);
        t.param(version);
        let address = t.builder.address(
            AddressBase::Slot(slot),
            &[],
            ValueRepr::Handle,
            owner_layout,
        );
        t.prepare_undo(support::ENTRY, address, &[]);
        let loan = t.loan(address, true);
        owner_views.push((address, loan, version));
    }
    let mut owners = Vec::new();
    for (address, loan, version) in &owner_views {
        t.begin(support::ENTRY, *loan);
        let result = t.value_as(ValueRepr::Handle, owner_layout, Holding::Borrowed(*loan));
        t.builder.push(
            support::ENTRY,
            Inst::Read {
                result,
                address: *address,
                reaching: *version,
                mode: ReadMode::Borrow,
            },
        );
        owners.push(result);
    }
    let loan = owner_views[0].1;
    let (repr, holding) = match kind {
        ViewRoot::Unit => (ValueRepr::Unit, Holding::Plain),
        ViewRoot::Scalar => (ValueRepr::Scalar(ScalarKind::U64), Holding::Plain),
        ViewRoot::Heap => (ValueRepr::Block, Holding::Borrowed(loan)),
    };
    let result = t.value_as(repr, root_layout, holding);
    t.builder.push(
        support::ENTRY,
        Inst::BorrowSealedRoot {
            result,
            owner: owners[0],
            loan,
            shape: SealDescriptorId(0),
        },
    );
    for (_, loan, _) in owner_views.into_iter().rev() {
        t.end(support::ENTRY, loan);
    }
    t.builder.ret(support::ENTRY, None);
    let d = Declarations {
        registry: t.operations.clone(),
        payloads: None,
        expected: Some(vec![SourceEvent {
            anchor: t.builder.unit.tables.source_anchors[0].clone(),
            kind: SourceEventKind::BorrowRoot,
            root_type: native.clone(),
        }]),
    };
    (t, d, result, loan, owners[1])
}

#[test]
fn sealed_root_views_accept_bits_and_owned_payloads() {
    for kind in [ViewRoot::Unit, ViewRoot::Scalar, ViewRoot::Heap] {
        let (t, d, _, _, _) = root_view_fixture(kind);
        assert_eq!(checked(t, &d), [], "root {kind:?}");
    }
}

#[test]
fn sealed_root_views_reject_owner_loan_and_holding_mutations() {
    use hirsel_lir::validate::ViolationKind as K;
    for kind in [ViewRoot::Unit, ViewRoot::Scalar, ViewRoot::Heap] {
        for mutation in 0..4 {
            let (t, d, result, loan, other_owner) = root_view_fixture(kind);
            let (mut unit, table, manifest) = t.finish();
            match mutation {
                0 => {
                    // The other owner has a valid active loan of its own.
                    // That loan cannot authorize reading this owner's root.
                    for inst in &mut unit.instructions {
                        if let Inst::BorrowSealedRoot { owner, .. } = inst {
                            *owner = other_owner;
                        }
                    }
                }
                1 => {
                    // Omit the loan activation; conditional ends cannot make
                    // either the owner read or the root read valid.
                    let begin = unit
                        .instructions
                        .iter()
                        .position(
                            |inst| matches!(inst, Inst::BeginLoan { loan: l, .. } if *l == loan),
                        )
                        .unwrap();
                    let end = unit
                        .instructions
                        .iter()
                        .position(
                            |inst| matches!(inst, Inst::EndLoan { loan: l, .. } if *l == loan),
                        )
                        .unwrap();
                    unit.instructions[begin] = Inst::EndLoan {
                        loan,
                        if_active: true,
                    };
                    unit.instructions[end] = Inst::EndLoan {
                        loan,
                        if_active: true,
                    };
                }
                2 => {
                    unit.tables.loans[loan.index()].mutable = false;
                    for inst in &mut unit.instructions {
                        if let Inst::BeginLoan {
                            loan: l, mutable, ..
                        } = inst
                            && *l == loan
                        {
                            *mutable = false;
                        }
                    }
                }
                _ => {
                    unit.values[result.index()].holding = match kind {
                        ViewRoot::Heap => Holding::Plain,
                        _ => Holding::Borrowed(loan),
                    };
                }
            }
            let errors = checked_raw(unit, &table, &manifest, &d);
            match mutation {
                0 => sealed_rejection(&errors, "exclusive owner loan"),
                1 => assert!(
                    errors
                        .iter()
                        .any(|error| matches!(error.kind, K::OutsideLoan(_))),
                    "{kind:?}: {errors:?}"
                ),
                2 => assert!(
                    errors.iter().any(|error| matches!(
                        error.kind,
                        K::Type("sealed owner requires an exclusive loan")
                    )),
                    "{kind:?}: {errors:?}"
                ),
                _ => assert!(
                    errors.iter().any(|error| matches!(
                        error.kind,
                        K::Holding(_) | K::Type("read-only sealed root loan")
                    )),
                    "{kind:?}: {errors:?}"
                ),
            }
        }
    }
}

#[test]
fn sealed_root_scalar_can_outlive_the_loan_but_a_heap_view_cannot_escape() {
    for kind in [ViewRoot::Scalar, ViewRoot::Heap] {
        let (mut t, d, result, _, _) = root_view_fixture(kind);
        let root_layout = t.builder.unit.tables.seal_descriptors[0].root_layout;
        let params: Vec<_> = t.table.fn_abis[t.builder.unit.header.abi.0 as usize]
            .parameters
            .iter()
            .map(|p| (p.convention, p.layout))
            .collect();
        t.builder.unit.header.abi = t.abi(&params, Some(root_layout));
        t.builder.ret(support::ENTRY, Some(result));
        let (unit, table, manifest) = t.finish();
        let target = TargetSpec::x86_64_unknown_linux_gnu();
        let inputs = hirsel_lir::validate::Inputs {
            target: &target,
            layouts: &table,
            manifest: &manifest,
            operations: &d,
            discharge: None,
            memo: Default::default(),
        };
        let errors = hirsel_lir::validate::check_scoped_transport(&unit, &inputs);
        match kind {
            ViewRoot::Scalar => {
                assert!(errors.is_empty(), "{errors:?}");
                assert!(hirsel_lir::validate::validate(unit, &inputs).is_ok());
            }
            ViewRoot::Heap => assert!(
                errors.iter().any(
                    |e| matches!(e.kind, hirsel_lir::validate::ViolationKind::ScopedRetention {
                    value, operation: "return or error payload", ..
                } if value == result)
                ),
                "{errors:?}"
            ),
            ViewRoot::Unit => unreachable!(),
        }
    }
}

#[test]
fn sealed_runtime_drop_33_descriptor_releases_exact_owner_with_shifted_offset() {
    let (t, _, _, _) = transition_fixture();
    let (unit, table, _) = t.finish();
    let owner = unit
        .instructions
        .iter()
        .find_map(|inst| {
            if let Inst::SealArena { result, .. } = inst {
                Some(*result)
            } else {
                None
            }
        })
        .expect("the original seal transition produces an owner");
    let owner_layout = unit.types[unit.values[owner.index()].ty.index()].layout;
    let descriptor = hirsel_lir::rt_program::descriptor(&table, owner_layout).unwrap();
    assert_eq!(descriptor.steps.len(), 1);
    let step = &descriptor.steps[0];
    assert_eq!(step.kind, hirsel_lir::rt_program::StepKind::ReleaseSealed);
    assert_eq!(
        step.kind.code(),
        hirsel_lir::runtime_abi::STEP_RELEASE_SEALED
    );
    assert_eq!(step.offset, 0);
    assert_eq!(step.field, hirsel_lir::runtime_abi::WHOLE_VALUE);
    assert!(step.layout.is_none());
    assert_eq!(
        hirsel_lir::layout::shift_drop(&table.layouts[owner_layout.0 as usize].drop[0], 24),
        hirsel_lir::layout_types::DropStep::ReleaseSealed { offset: 24 }
    );
    descriptor.check_finalizers(&table).unwrap();
}

#[test]
fn sealed_native_cache_33_replays_independent_source_and_descriptor_reads() {
    use hirsel_artifact_store::Store;
    use hirsel_lir::sealed_payload_source::SealSourceWitness;
    use hirsel_lir::stage::{Ready, StageKind};
    use hirsel_lir::validate::{Inputs, NativeCache, NativeEnvironment};
    use std::sync::Arc;
    struct Selected {
        declarations: Declarations,
        source: hirsel_syntax::ContentHash,
        alter_descriptor: bool,
        witness: bool,
        fixed_witness: Option<SealSourceWitness>,
        payloads: Vec<hirsel_lir::sealed_payload_source::SourcePayload>,
    }
    impl OperationDeclarations for Selected {
        fn derive(
            &self,
            op: &OperationId,
            args: &[OperationArgument],
            target: &TargetSpec,
        ) -> Result<(hirsel_lir::FnAbi, LayoutTable), hirsel_lir::layout::LayoutError> {
            self.declarations.derive(op, args, target)
        }
        fn seal_events(
            &self,
            d: &hirsel_syntax::DefPath,
            i: hirsel_syntax::ContentHash,
        ) -> Option<Vec<SourceEvent>> {
            self.declarations.seal_events(d, i)
        }
        fn seal_payloads(
            &self,
            _: &hirsel_syntax::DefPath,
            _: hirsel_syntax::ContentHash,
        ) -> Option<Vec<hirsel_lir::sealed_payload_source::SourcePayload>> {
            Some(self.payloads.clone())
        }
        fn seal_source_witness(
            &self,
            d: &hirsel_syntax::DefPath,
            i: hirsel_syntax::ContentHash,
        ) -> Option<SealSourceWitness> {
            if !self.witness || *d != support::path("f") || i != support::hash("f instance") {
                return None;
            }
            if let Some(witness) = &self.fixed_witness {
                return Some(witness.clone());
            }
            Some(SealSourceWitness::from_expectations(
                self.source,
                support::path("f"),
                support::hash("f source"),
                support::hash("f instance"),
                self.declarations.expected.as_ref()?,
                &self.payloads,
            ))
        }
        fn seal_descriptor_for_source(
            &self,
            d: &hirsel_syntax::DefPath,
            i: hirsel_syntax::ContentHash,
            root: &NativeType,
            table: &LayoutTable,
            target: &TargetSpec,
        ) -> Result<hirsel_lir::sealed::SealDescriptor, hirsel_lir::layout::LayoutError> {
            let mut descriptor = self
                .declarations
                .seal_descriptor_for_source(d, i, root, table, target)?;
            if self.alter_descriptor {
                descriptor.logical.facts[0].bytes = LogicalBytes::Bytes(99);
            }
            Ok(descriptor)
        }
    }
    let (t, declarations, _, _) = transition_fixture();
    let (unit, table, manifest) = t.finish();
    let unit = unit.into_stage::<Ready>();
    let mut selected = Selected {
        declarations,
        source: support::hash("independent source query"),
        alter_descriptor: false,
        witness: true,
        fixed_witness: None,
        payloads: vec![],
    };
    let target = TargetSpec::x86_64_unknown_linux_gnu();
    let root = std::path::Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("sealed-native-cache-33-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let local = Store::new(root.clone());
    let cache = NativeCache::new(Arc::new(local.clone()), Some(local.clone()));
    let subject = support::hash("sealed cache subject");
    let policy = support::hash("sealed cache policy");
    let inputs = Inputs {
        target: &target,
        layouts: &table,
        manifest: &manifest,
        operations: &selected,
        discharge: None,
        memo: Default::default(),
    };
    let (_, output) = cache
        .validate_and_publish(subject, policy, unit.clone(), &inputs, &[], &[])
        .unwrap();
    let output = output.expect("actual sealed C must capture its independent declaration reads");
    drop(cache);
    // A fresh authority table must decode and recheck all durable sealed reads.
    let cache = NativeCache::new(Arc::new(local.clone()), Some(local));
    let lookup = |selected: &Selected| {
        cache.lookup(
            subject,
            policy,
            StageKind::Ready,
            &NativeEnvironment {
                target: &target,
                manifest: &manifest,
                operations: selected,
                discharge: None,
            },
        )
    };
    let trace = hirsel_lir::construction::ValueTrace::default();
    let observe = trace.enter();
    assert!(
        lookup(&selected).is_some(),
        "portable layout rebinding preserves independently derived descriptor identity"
    );
    selected.alter_descriptor = true;
    assert!(lookup(&selected).is_none());
    selected.alter_descriptor = false;
    selected.source = support::hash("changed independent source query");
    assert!(lookup(&selected).is_none());
    selected.source = support::hash("independent source query");
    selected.fixed_witness =
        selected.seal_source_witness(&support::path("f"), support::hash("f instance"));
    selected.declarations.expected.as_mut().unwrap()[0]
        .anchor
        .statement += 1;
    assert!(lookup(&selected).is_none());
    selected.declarations.expected.as_mut().unwrap()[0]
        .anchor
        .statement -= 1;
    selected
        .payloads
        .push(hirsel_lir::sealed_payload_source::SourcePayload {
            root_type: NativeType::Primitive(hirsel_lir::operation::Primitive::U64),
            recipe: hirsel_lir::sealed_payload_source::PayloadRecipe {
                source: selected.declarations.expected.as_ref().unwrap()[0]
                    .anchor
                    .clone(),
                operation: hirsel_lir::sealed_payload_source::PayloadOperation::Boundary,
                inputs: vec![],
            },
        });
    assert!(
        lookup(&selected).is_none(),
        "a stale witness cannot hide changed payload expectations"
    );
    selected.payloads.clear();
    assert!(lookup(&selected).is_some());
    selected.declarations.expected = None;
    assert!(
        lookup(&selected).is_none(),
        "available source inventory cannot become unavailable"
    );
    assert_eq!(trace.totals(), Default::default());
    drop(observe);
    let (_, declarations, _, _) = transition_fixture();
    selected.declarations = declarations;
    selected.witness = false;
    selected.fixed_witness = None;
    let inputs = Inputs {
        target: &target,
        layouts: &table,
        manifest: &manifest,
        operations: &selected,
        discharge: None,
        memo: Default::default(),
    };
    assert!(
        cache
            .validate_output(
                support::hash("missing independent witness"),
                policy,
                unit,
                &inputs,
                &[]
            )
            .unwrap()
            .1
            .is_none()
    );
    assert_eq!(output.stage(), StageKind::Ready);
    let _ = std::fs::remove_dir_all(root);
}
